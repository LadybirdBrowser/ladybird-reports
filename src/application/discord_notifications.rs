use std::time::Duration;

use chrono::SecondsFormat;
use serde_json::Value;

use crate::{
    domain::DiscordConfiguration,
    infrastructure::{
        database::{AdminDatabase, PendingDiscordNotification},
        discord::{
            DiscordAllowedMentions, DiscordClient, DiscordDeliveryFailure, DiscordEmbed,
            DiscordEmbedField, DiscordWebhookMessage,
        },
    },
};

#[derive(Clone)]
pub struct DiscordNotificationService {
    database: AdminDatabase,
    client: DiscordClient,
}

impl DiscordNotificationService {
    pub fn new(database: AdminDatabase, client: DiscordClient) -> Self {
        Self { database, client }
    }

    pub async fn run(self) {
        loop {
            let delay = self.dispatch_once().await;
            if delay.is_zero() {
                tokio::task::yield_now().await;
            } else {
                tokio::time::sleep(delay).await;
            }
        }
    }

    async fn dispatch_once(&self) -> Duration {
        let configuration = match self.database.configuration().await {
            Ok(configuration) => configuration,
            Err(error) => {
                tracing::warn!(event = "discord.configuration_failed", ?error);
                return Duration::from_secs(60);
            }
        };
        let discord = &configuration.discord;
        let poll_delay = Duration::from_secs(discord.poll_interval_seconds);
        let Some(webhook_url) = discord.webhook_url.as_deref() else {
            return poll_delay;
        };

        let lease_seconds = discord.request_timeout_seconds.saturating_add(30);
        let notification = match self
            .database
            .claim_discord_notification(lease_seconds)
            .await
        {
            Ok(Some(notification)) => notification,
            Ok(None) => return poll_delay,
            Err(error) => {
                tracing::warn!(event = "discord.queue_claim_failed", ?error);
                return poll_delay;
            }
        };

        let message = report_message(&notification, &configuration.admin_base_url, discord);
        let result = self
            .client
            .send(
                webhook_url,
                &message,
                Duration::from_secs(discord.request_timeout_seconds),
            )
            .await;

        match result {
            Ok(receipt) => {
                match self
                    .database
                    .finish_discord_notification(
                        notification.report_id,
                        notification.lease_id,
                        receipt.cooldown_seconds,
                    )
                    .await
                {
                    Ok(true) => tracing::info!(
                        event = "discord.report_delivered",
                        report_id = %notification.report_id,
                    ),
                    Ok(false) => tracing::warn!(
                        event = "discord.delivery_lease_lost",
                        report_id = %notification.report_id,
                    ),
                    Err(error) => tracing::warn!(
                        event = "discord.delivery_record_failed",
                        report_id = %notification.report_id,
                        ?error,
                    ),
                }

                Duration::from_secs(receipt.cooldown_seconds)
            }
            Err(failure) => {
                self.defer_failed_delivery(&notification, discord, failure)
                    .await
            }
        }
    }

    async fn defer_failed_delivery(
        &self,
        notification: &PendingDiscordNotification,
        configuration: &DiscordConfiguration,
        failure: DiscordDeliveryFailure,
    ) -> Duration {
        let delay_seconds = retry_delay_seconds(
            configuration,
            notification.attempt_count,
            failure.retry_after_seconds,
        );
        match self
            .database
            .defer_discord_notification(
                notification.report_id,
                notification.lease_id,
                delay_seconds,
                failure.response_status,
                failure.category,
            )
            .await
        {
            Ok(true) => tracing::warn!(
                event = "discord.delivery_deferred",
                report_id = %notification.report_id,
                category = failure.category,
                response_status = failure.response_status,
                retry_seconds = delay_seconds,
            ),
            Ok(false) => tracing::warn!(
                event = "discord.delivery_lease_lost",
                report_id = %notification.report_id,
            ),
            Err(error) => tracing::warn!(
                event = "discord.delivery_defer_failed",
                report_id = %notification.report_id,
                ?error,
            ),
        }

        Duration::from_secs(delay_seconds)
    }
}

fn retry_delay_seconds(
    configuration: &DiscordConfiguration,
    attempt_count: u32,
    server_delay: Option<u64>,
) -> u64 {
    let multiplier = 1_u64.checked_shl(attempt_count.min(20)).unwrap_or(u64::MAX);
    let exponential_delay = configuration
        .initial_retry_seconds
        .saturating_mul(multiplier)
        .min(configuration.maximum_retry_seconds);

    exponential_delay.max(server_delay.unwrap_or(0))
}

fn report_message(
    notification: &PendingDiscordNotification,
    admin_base_url: &str,
    configuration: &DiscordConfiguration,
) -> DiscordWebhookMessage {
    let report_url = format!(
        "{}/reports/{}",
        admin_base_url.trim_end_matches('/'),
        notification.report_id
    );
    let fields = notification
        .fields
        .as_object()
        .expect("database aggregates diagnostic fields into an object");
    let description = fields
        .get("stack")
        .and_then(Value::as_str)
        .map(|stack| stack_description(stack, configuration))
        .or_else(|| {
            fields
                .get("description")
                .and_then(Value::as_str)
                .map(|description| {
                    format!(
                        "**Description**\n{}",
                        truncate_text(description, 2_000).replace('`', "′")
                    )
                })
        })
        .unwrap_or_else(|| "A new report is ready for triage.".into());

    let mut embed_fields = vec![DiscordEmbedField {
        name: "Version".into(),
        value: truncate_text(&notification.client_version, 256),
        inline: true,
    }];
    if !notification.build.trim().is_empty() {
        embed_fields.push(DiscordEmbedField {
            name: "Build".into(),
            value: truncate_text(&notification.build, 512),
            inline: true,
        });
    }
    for (key, label) in [
        ("platform", "Platform"),
        ("architecture", "Architecture"),
        ("signal", "Signal"),
        ("signal_number", "Signal number"),
    ] {
        if let Some(value) = fields.get(key) {
            let value = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            embed_fields.push(DiscordEmbedField {
                name: label.into(),
                value: truncate_text(&value, 256),
                inline: true,
            });
        }
    }

    DiscordWebhookMessage {
        username: "Ladybird Reports",
        embeds: vec![DiscordEmbed {
            title: truncate_text(&notification.title, 256),
            url: report_url,
            description,
            color: report_color(&notification.kind),
            fields: embed_fields,
            timestamp: notification
                .created_at
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        }],
        allowed_mentions: DiscordAllowedMentions { parse: Vec::new() },
    }
}

fn stack_description(stack: &str, configuration: &DiscordConfiguration) -> String {
    let selected_lines = stack
        .lines()
        .take(configuration.stack_trace_lines)
        .collect::<Vec<_>>();
    let selected = selected_lines.join("\n").replace('`', "′");
    let was_truncated = stack.lines().count() > selected_lines.len()
        || selected.chars().count() > configuration.stack_trace_characters;
    let mut excerpt = truncate_text(&selected, configuration.stack_trace_characters);
    if was_truncated && !excerpt.ends_with('…') {
        excerpt.push('…');
    }

    format!("**Stack trace (excerpt)**\n```text\n{excerpt}\n```")
}

fn truncate_text(value: &str, maximum_characters: usize) -> String {
    if value.chars().count() <= maximum_characters {
        return value.to_owned();
    }

    let mut truncated = value
        .chars()
        .take(maximum_characters.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn report_color(kind: &str) -> u32 {
    match kind {
        "crash" => 0xd84a4a,
        "web_compat" => 0x5d55dc,
        _ => 0x666b7a,
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::{
        domain::{DiscordConfiguration, DiscordDeliveryLeaseId, ReportId},
        infrastructure::database::PendingDiscordNotification,
    };

    use super::{report_message, retry_delay_seconds};

    #[test]
    fn report_message_includes_context_and_a_bounded_stack_excerpt() {
        let configuration = DiscordConfiguration {
            stack_trace_lines: 2,
            stack_trace_characters: 256,
            ..DiscordConfiguration::default()
        };
        let notification = PendingDiscordNotification {
            report_id: ReportId::new(),
            lease_id: DiscordDeliveryLeaseId::new(),
            title: "Crash: WebContent::ConnectionFromClient::debug_request".into(),
            kind: "crash".into(),
            client_version: "Ladybird Nightly".into(),
            build: "macOS arm64".into(),
            fields: serde_json::json!({
                "platform": "macOS",
                "architecture": "arm64",
                "signal": "SIGABRT",
                "signal_number": 6,
                "stack": "frame one\nframe `two`\nframe three"
            }),
            created_at: Utc::now(),
            attempt_count: 0,
        };

        let message = report_message(&notification, "https://reports.example", &configuration);
        let embed = &message.embeds[0];

        assert_eq!(
            embed.title,
            "Crash: WebContent::ConnectionFromClient::debug_request"
        );
        assert!(embed.url.ends_with(&notification.report_id.to_string()));
        assert!(embed.description.contains("frame one"));
        assert!(embed.description.contains("frame ′two′"));
        assert!(!embed.description.contains("frame three"));
        assert!(embed.description.contains('…'));
        assert!(
            embed.fields.iter().any(|field| {
                field.name == "Platform" && field.value == "macOS" && field.inline
            })
        );
        assert!(
            embed
                .fields
                .iter()
                .any(|field| field.name == "Signal number" && field.value == "6")
        );
        assert!(message.allowed_mentions.parse.is_empty());
    }

    #[test]
    fn retry_delay_is_exponential_and_honors_discord() {
        let configuration = DiscordConfiguration::default();

        assert_eq!(retry_delay_seconds(&configuration, 0, None), 30);
        assert_eq!(retry_delay_seconds(&configuration, 3, None), 240);
        assert_eq!(retry_delay_seconds(&configuration, 20, None), 3_600);
        assert_eq!(retry_delay_seconds(&configuration, 0, Some(4_000)), 4_000);
    }
}
