use std::time::Duration;

use chrono::SecondsFormat;
use serde_json::Value;

use crate::{
    domain::{DiscordConfiguration, ReportKind, concise_function_name, parse_stack_trace},
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
    for (key, label) in [("platform", "Platform"), ("architecture", "Architecture")] {
        if let Some(value) = fields.get(key) {
            embed_fields.push(DiscordEmbedField {
                name: label.into(),
                value: truncate_text(&field_text(value), 256),
                inline: true,
            });
        }
    }

    embed_fields.extend(signal_field(fields));
    embed_fields.extend(page_url_field(fields));

    DiscordWebhookMessage {
        username: "Ladybird Reports",
        embeds: vec![DiscordEmbed {
            title: truncate_text(&notification.title, 256),
            url: report_url,
            description,
            color: report_color(notification.kind),
            fields: embed_fields,
            timestamp: notification
                .created_at
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        }],
        allowed_mentions: DiscordAllowedMentions { parse: Vec::new() },
    }
}

fn field_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

/// The signal that ended the process, by name and number as in `SIGSEGV (11)`.
/// A report may carry only one of the two.
fn signal_field(fields: &serde_json::Map<String, Value>) -> Option<DiscordEmbedField> {
    let text = |key: &str, maximum_characters: usize| {
        let text = field_text(fields.get(key)?);
        let text = text.trim();
        (!text.is_empty()).then(|| truncate_text(text, maximum_characters))
    };

    let value = match (text("signal", 200), text("signal_number", 32)) {
        (Some(name), Some(number)) => format!("{name} ({number})"),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => return None,
    };

    Some(DiscordEmbedField {
        name: "Signal".into(),
        value,
        inline: true,
    })
}

/// The page a report came from, without its query or fragment, which can carry
/// tokens or personal data. It is shown as code so that Discord does not turn an
/// address chosen by an anonymous user into a link anyone in the channel can
/// click; a maintainer who wants to visit it copies it deliberately.
fn page_url_field(fields: &serde_json::Map<String, Value>) -> Option<DiscordEmbedField> {
    // Discord limits an embed field to 1024 characters, and the quotes need two.
    const MAX_URL_CHARACTERS: usize = 1_000;

    let url = fields.get("url")?.as_str()?;
    let url = url.split(['?', '#']).next().unwrap_or_default().trim();
    if url.is_empty() {
        return None;
    }

    let url = url
        .chars()
        .map(|character| match character {
            '`' => '′',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect::<String>();

    Some(DiscordEmbedField {
        name: "Page URL".into(),
        value: format!("`{}`", truncate_text(&url, MAX_URL_CHARACTERS)),
        inline: false,
    })
}

/// One line of the stack preview: a frame, or a run of consecutive frames the
/// client sent no symbol for.
struct PreviewLine {
    first: u32,
    last: u32,
    symbol: Option<String>,
}

impl PreviewLine {
    fn frame_count(&self) -> usize {
        (self.last - self.first) as usize + 1
    }

    fn render(&self) -> String {
        let frames = if self.first == self.last {
            format!("#{}", self.first)
        } else {
            format!("#{}–#{}", self.first, self.last)
        };

        format!(
            "`{frames}` `{}`\n",
            self.symbol.as_deref().unwrap_or("Unavailable")
        )
    }
}

fn stack_description(stack: &str, configuration: &DiscordConfiguration) -> String {
    const MAX_FUNCTION_CHARACTERS: usize = 96;
    const MORE_FRAMES: &str = "*More frames in the full report.*";

    let parsed = parse_stack_trace(stack);
    let frames = parsed
        .rows
        .iter()
        .filter(|row| row.number.is_some())
        .collect::<Vec<_>>();

    if frames.is_empty() {
        return raw_stack_description(stack, configuration);
    }

    let has_relevant_frames = frames.iter().any(|row| row.relevant);
    let mut lines: Vec<PreviewLine> = Vec::new();

    for frame in &frames {
        if has_relevant_frames && !frame.top && !frame.relevant {
            continue;
        }

        let number = frame.number.unwrap();
        if frame.unavailable
            && let Some(run) = lines
                .last_mut()
                .filter(|run| run.symbol.is_none() && run.last + 1 == number)
        {
            run.last = number;
            continue;
        }

        let symbol = if frame.unavailable {
            None
        } else {
            Some(
                concise_function_name(&frame.symbol, MAX_FUNCTION_CHARACTERS).unwrap_or_else(
                    || truncate_text(&frame.symbol.replace('`', "′"), MAX_FUNCTION_CHARACTERS),
                ),
            )
        };
        lines.push(PreviewLine {
            first: number,
            last: number,
            symbol,
        });
    }

    let mut description = String::from("**Stack trace**\n");
    let mut displayed_lines = 0;
    let mut displayed_frames = 0;

    for line in &lines {
        if displayed_lines >= configuration.stack_trace_lines {
            break;
        }

        let text = line.render();
        if description.chars().count() + text.chars().count() + MORE_FRAMES.chars().count()
            > configuration.stack_trace_characters
        {
            break;
        }
        description.push_str(&text);
        displayed_lines += 1;
        displayed_frames += line.frame_count();
    }

    if displayed_lines == 0 {
        return raw_stack_description(stack, configuration);
    }

    if parsed.frame_count > displayed_frames || parsed.truncated {
        description.push_str(MORE_FRAMES);
    } else {
        description.pop();
    }

    description
}

fn raw_stack_description(stack: &str, configuration: &DiscordConfiguration) -> String {
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

    format!("**Stack trace**\n```text\n{excerpt}\n```")
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

fn report_color(kind: ReportKind) -> u32 {
    match kind {
        ReportKind::Crash => 0xd84a4a,
        ReportKind::WebCompat => 0x5d55dc,
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::{
        domain::{DiscordConfiguration, DiscordDeliveryLeaseId, ReportId, ReportKind},
        infrastructure::database::PendingDiscordNotification,
    };

    use super::{page_url_field, report_message, retry_delay_seconds, stack_description};

    #[test]
    fn parsed_stack_preview_shows_functions_without_native_metadata() {
        let stack = "Native stack (binary build ID, object address):\n\
            #0 4402e9f4aa8030998b5a8e8bab39a036 0x100028897 non-virtual thunk to Compositor::ConnectionFromClient::crash() at Build/release/bin/Compositor\n\
            #1 4402e9f4aa8030998b5a8e8bab39a036 0x10002b943 CompositorControlServerStub::handle_crash() at Build/release/bin/Compositor\n\
            #2 4402e9f4aa8030998b5a8e8bab39a036 0x10002b944 Core::ThreadEventQueue::process() at Build/release/lib/liblagom-core.dylib";
        let preview = stack_description(stack, &DiscordConfiguration::default());

        assert!(preview.contains("`#0` `Compositor::ConnectionFromClient::crash`"));
        assert!(preview.contains("`#1` `CompositorControlServerStub::handle_crash`"));
        assert!(preview.contains("More frames in the full report"));
        assert!(!preview.contains("4402e9f4"));
        assert!(!preview.contains("0x100028897"));
        assert!(!preview.contains("Build/release"));
        assert!(!preview.contains("Core::ThreadEventQueue"));
    }

    #[test]
    fn preview_obeys_configured_frame_and_character_limits() {
        let stack = "#0 0x100 WebContent::Page::load() at WebContent\n\
            #1 0x200 WebContent::Page::layout() at WebContent\n\
            #2 0x300 WebContent::Page::paint() at WebContent";
        let configuration = DiscordConfiguration {
            stack_trace_lines: 1,
            stack_trace_characters: 256,
            ..DiscordConfiguration::default()
        };
        let preview = stack_description(stack, &configuration);

        assert!(preview.contains("`#0` `WebContent::Page::load`"));
        assert!(!preview.contains("Page::layout"));
        assert!(preview.chars().count() <= configuration.stack_trace_characters);
    }

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
            kind: ReportKind::Crash,
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
                .any(|field| field.name == "Signal" && field.value == "SIGABRT (6)")
        );
        assert!(
            embed
                .fields
                .iter()
                .all(|field| field.name != "Signal number")
        );
        assert!(message.allowed_mentions.parse.is_empty());
    }

    fn notification_with(fields: serde_json::Value) -> PendingDiscordNotification {
        PendingDiscordNotification {
            report_id: ReportId::new(),
            lease_id: DiscordDeliveryLeaseId::new(),
            title: "Web compatibility report".into(),
            kind: ReportKind::WebCompat,
            client_version: "Ladybird Nightly".into(),
            build: String::new(),
            fields,
            created_at: Utc::now(),
            attempt_count: 0,
        }
    }

    fn signal_in_message(fields: serde_json::Value) -> Option<String> {
        let message = report_message(
            &notification_with(fields),
            "https://reports.example",
            &DiscordConfiguration::default(),
        );
        let mut signals = message.embeds[0]
            .fields
            .iter()
            .filter(|field| field.name.starts_with("Signal"));
        let signal = signals.next().map(|field| field.value.clone());

        assert!(signals.next().is_none(), "the signal is a single field");
        signal
    }

    #[test]
    fn report_message_combines_the_signal_name_and_number() {
        assert_eq!(
            signal_in_message(serde_json::json!({ "signal": "SIGSEGV", "signal_number": 11 })),
            Some("SIGSEGV (11)".into())
        );
        assert_eq!(
            signal_in_message(serde_json::json!({ "signal": "SIGSEGV", "signal_number": "11" })),
            Some("SIGSEGV (11)".into())
        );
    }

    #[test]
    fn report_message_shows_whichever_part_of_the_signal_is_present() {
        assert_eq!(
            signal_in_message(serde_json::json!({ "signal": "SIGABRT" })),
            Some("SIGABRT".into())
        );
        assert_eq!(
            signal_in_message(serde_json::json!({ "signal_number": 6 })),
            Some("6".into())
        );
        assert_eq!(
            signal_in_message(serde_json::json!({ "signal": " ", "signal_number": 6 })),
            Some("6".into())
        );
        assert_eq!(signal_in_message(serde_json::json!({})), None);
    }

    #[test]
    fn a_long_signal_name_keeps_the_number() {
        let name = "SIG".repeat(200);
        let signal = signal_in_message(serde_json::json!({ "signal": name, "signal_number": 11 }))
            .expect("signal field");

        assert!(signal.ends_with("… (11)"));
        assert!(signal.chars().count() <= 256);
    }

    fn preview(stack: &str) -> String {
        stack_description(stack, &DiscordConfiguration::default())
    }

    const BUILD: &str = "3a03ed2b8ce032fa99c3ca574ad081ad";

    #[test]
    fn a_stack_without_symbols_is_one_line() {
        let stack = (0..128)
            .map(|number| format!("#{number} {BUILD} 0x{:x}", 0x1000 + number))
            .collect::<Vec<_>>()
            .join("\n");
        let text = format!("Native stack (binary build ID, object address):\n{stack}");

        assert_eq!(preview(&text), "**Stack trace**\n`#0–#127` `Unavailable`");
    }

    #[test]
    fn unavailable_frames_collapse_per_run_and_stay_apart_from_named_frames() {
        let text = format!(
            "#0 {BUILD} 0x1\n#1 {BUILD} 0x2\n#2 {BUILD} 0x3 main\n\
             #3 {BUILD} 0x4\n#4 {BUILD} 0x5\n#5 {BUILD} 0x6"
        );

        assert_eq!(
            preview(&text),
            "**Stack trace**\n`#0–#1` `Unavailable`\n`#2` `main`\n`#3–#5` `Unavailable`"
        );
    }

    #[test]
    fn a_single_unavailable_frame_keeps_its_own_number() {
        let text = format!("#0 {BUILD} 0x1\n#1 {BUILD} 0x2 main\n#2 unavailable");

        assert_eq!(
            preview(&text),
            "**Stack trace**\n`#0` `Unavailable`\n`#1` `main`\n`#2` `Unavailable`"
        );
    }

    #[test]
    fn frames_left_out_of_the_preview_still_say_there_is_more() {
        let text = format!(
            "#0 {BUILD} 0x1\n#1 {BUILD} 0x2\n#2 {BUILD} 0x3 Web::Foo::bar()\n#3 {BUILD} 0x4"
        );
        let preview = preview(&text);

        assert!(preview.contains("`#0` `Unavailable`"));
        assert!(preview.contains("`#2` `Web::Foo::bar`"));
        assert!(!preview.contains("`#1`"));
        assert!(preview.ends_with("*More frames in the full report.*"));
    }

    #[test]
    fn a_collapsed_run_counts_as_one_line_of_the_preview() {
        let configuration = DiscordConfiguration {
            stack_trace_lines: 2,
            ..DiscordConfiguration::default()
        };
        let text = format!(
            "#0 {BUILD} 0x1\n#1 {BUILD} 0x2\n#2 {BUILD} 0x3\n#3 {BUILD} 0x4 main\n\
             #4 {BUILD} 0x5 _start\n#5 {BUILD} 0x6"
        );

        assert_eq!(
            stack_description(&text, &configuration),
            "**Stack trace**\n`#0–#2` `Unavailable`\n`#3` `main`\n*More frames in the full report.*"
        );
    }

    fn page_url_in_message(fields: serde_json::Value) -> Option<String> {
        let message = report_message(
            &notification_with(fields),
            "https://reports.example",
            &DiscordConfiguration::default(),
        );

        message.embeds[0]
            .fields
            .iter()
            .find(|field| field.name == "Page URL")
            .map(|field| {
                assert!(!field.inline, "a long address needs the full width");
                field.value.clone()
            })
    }

    #[test]
    fn report_message_includes_the_page_url_when_the_report_has_one() {
        assert_eq!(
            page_url_in_message(serde_json::json!({ "url": "https://example.test/a/b" })),
            Some("`https://example.test/a/b`".into())
        );
    }

    #[test]
    fn page_url_leaves_out_the_query_and_fragment() {
        for (url, shown) in [
            (
                "https://example.test/a?token=secret",
                "https://example.test/a",
            ),
            ("https://example.test/a#section", "https://example.test/a"),
            ("https://example.test/a?b=1#c", "https://example.test/a"),
            (
                "https://example.test/#/route?token=secret",
                "https://example.test/",
            ),
            ("https://example.test?token=secret", "https://example.test"),
            ("https://example.test/a?", "https://example.test/a"),
        ] {
            assert_eq!(
                page_url_in_message(serde_json::json!({ "url": url })),
                Some(format!("`{shown}`")),
                "{url}"
            );
        }
    }

    #[test]
    fn page_url_cannot_produce_a_website_preview() {
        let url = "https://example.test/preview-me";
        let message = report_message(
            &notification_with(serde_json::json!({ "url": url })),
            "https://reports.example",
            &DiscordConfiguration::default(),
        );
        let json = serde_json::to_value(&message).expect("serialize message");

        // Discord only previews links in the message content, so there is none.
        let keys = json
            .as_object()
            .expect("message is an object")
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            ["allowed_mentions", "embeds", "username"]
                .into_iter()
                .collect()
        );

        // Inside the embed it appears once, as code, which Discord never links.
        let text = json.to_string();
        assert_eq!(text.matches(url).count(), 1);
        assert!(text.contains(&format!("`{url}`")));
    }

    #[test]
    fn report_message_omits_a_missing_or_empty_page_url() {
        for fields in [
            serde_json::json!({}),
            serde_json::json!({ "url": "" }),
            serde_json::json!({ "url": "  \n " }),
            serde_json::json!({ "url": "?token=secret" }),
            serde_json::json!({ "url": "#fragment" }),
            serde_json::json!({ "url": 42 }),
            serde_json::json!({ "hostname": "example.test" }),
        ] {
            assert_eq!(page_url_in_message(fields.clone()), None, "{fields}");
        }
    }

    #[test]
    fn page_url_cannot_break_out_of_its_code_quote_or_exceed_the_field_limit() {
        let hostile = page_url_field(
            serde_json::json!({ "url": "https://example.test/`**x**`\nsecond line\t" })
                .as_object()
                .expect("object"),
        )
        .expect("field");
        assert_eq!(hostile.value, "`https://example.test/′**x**′ second line`");

        let long = page_url_field(
            serde_json::json!({ "url": format!("https://example.test/{}", "a".repeat(5_000)) })
                .as_object()
                .expect("object"),
        )
        .expect("field");
        assert!(long.value.chars().count() <= 1_024);
        assert!(long.value.starts_with('`') && long.value.ends_with('`'));
        assert!(long.value.contains('…'));
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
