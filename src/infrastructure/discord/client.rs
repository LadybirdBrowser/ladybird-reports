use std::time::Duration;

use reqwest::{Client, Response, header::HeaderMap};
use serde::Serialize;

use crate::error::{AppError, Result};

const MAX_DELAY_SECONDS: u64 = i32::MAX as u64;

#[derive(Clone)]
pub struct DiscordClient {
    http: Client,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiscordWebhookMessage {
    pub username: &'static str,
    pub embeds: Vec<DiscordEmbed>,
    pub allowed_mentions: DiscordAllowedMentions,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiscordEmbed {
    pub title: String,
    pub url: String,
    pub description: String,
    pub color: u32,
    pub fields: Vec<DiscordEmbedField>,
    pub timestamp: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiscordEmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiscordAllowedMentions {
    pub parse: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiscordDeliveryReceipt {
    pub cooldown_seconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiscordDeliveryFailure {
    pub category: &'static str,
    pub response_status: Option<u16>,
    pub retry_after_seconds: Option<u64>,
}

impl DiscordClient {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .user_agent("Ladybird-Reports/0.1")
            .build()
            .map_err(|error| AppError::Internal(error.into()))?;

        Ok(Self { http })
    }

    pub async fn send(
        &self,
        webhook_url: &str,
        message: &DiscordWebhookMessage,
        timeout: Duration,
    ) -> std::result::Result<DiscordDeliveryReceipt, DiscordDeliveryFailure> {
        let mut webhook_url =
            reqwest::Url::parse(webhook_url).map_err(|_| DiscordDeliveryFailure {
                category: "configuration",
                response_status: None,
                retry_after_seconds: None,
            })?;
        webhook_url.query_pairs_mut().append_pair("wait", "true");

        let response = self
            .http
            .post(webhook_url)
            .timeout(timeout)
            .json(message)
            .send()
            .await
            .map_err(|error| DiscordDeliveryFailure {
                category: if error.is_timeout() {
                    "timeout"
                } else if error.is_connect() {
                    "connection"
                } else {
                    "request"
                },
                response_status: None,
                retry_after_seconds: None,
            })?;

        decode_response(response)
    }
}

fn decode_response(
    response: Response,
) -> std::result::Result<DiscordDeliveryReceipt, DiscordDeliveryFailure> {
    let status = response.status();
    let retry_after_seconds = duration_header(response.headers(), "retry-after");

    if !status.is_success() {
        return Err(DiscordDeliveryFailure {
            category: "response",
            response_status: Some(status.as_u16()),
            retry_after_seconds,
        });
    }

    let exhausted = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok())
        == Some("0");
    let cooldown_seconds = if exhausted {
        duration_header(response.headers(), "x-ratelimit-reset-after").unwrap_or(1)
    } else {
        0
    };

    Ok(DiscordDeliveryReceipt { cooldown_seconds })
}

fn duration_header(headers: &HeaderMap, name: &str) -> Option<u64> {
    let seconds = headers.get(name)?.to_str().ok()?.parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }

    Some((seconds.ceil().max(1.0) as u64).min(MAX_DELAY_SECONDS))
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, HeaderValue, StatusCode, Uri},
        routing::post,
    };
    use tokio::sync::Mutex;

    use super::{
        DiscordAllowedMentions, DiscordClient, DiscordDeliveryFailure, DiscordEmbed,
        DiscordWebhookMessage, duration_header,
    };

    #[test]
    fn discord_retry_delays_round_up_fractional_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("1.25"));

        assert_eq!(duration_header(&headers, "retry-after"), Some(2));
    }

    #[tokio::test]
    async fn webhook_delivery_waits_for_confirmation() {
        let requested_uri = Arc::new(Mutex::new(None));
        let server_state = requested_uri.clone();
        let app = Router::new()
            .route(
                "/api/webhooks/123/token",
                post(
                    |State(requested_uri): State<Arc<Mutex<Option<Uri>>>>,
                     uri: Uri,
                     Json(_message): Json<serde_json::Value>| async move {
                        *requested_uri.lock().await = Some(uri);
                        StatusCode::OK
                    },
                ),
            )
            .with_state(server_state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test webhook server");
        let address = listener.local_addr().expect("read test server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve test webhook request");
        });

        DiscordClient::new()
            .expect("create Discord client")
            .send(
                &format!("http://{address}/api/webhooks/123/token"),
                &test_message(),
                Duration::from_secs(2),
            )
            .await
            .expect("deliver test webhook");

        let requested_uri = requested_uri
            .lock()
            .await
            .clone()
            .expect("webhook request was received");
        assert_eq!(requested_uri.query(), Some("wait=true"));
        server.abort();
    }

    #[tokio::test]
    async fn webhook_failure_preserves_discord_retry_delay() {
        let app = Router::new().route(
            "/api/webhooks/123/token",
            post(|| async { (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "1.25")]) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test webhook server");
        let address = listener.local_addr().expect("read test server address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve test webhook request");
        });

        let failure = DiscordClient::new()
            .expect("create Discord client")
            .send(
                &format!("http://{address}/api/webhooks/123/token"),
                &test_message(),
                Duration::from_secs(2),
            )
            .await
            .expect_err("429 response must defer delivery");

        assert_eq!(
            failure,
            DiscordDeliveryFailure {
                category: "response",
                response_status: Some(429),
                retry_after_seconds: Some(2),
            }
        );
        server.abort();
    }

    fn test_message() -> DiscordWebhookMessage {
        DiscordWebhookMessage {
            username: "Ladybird Reports",
            embeds: vec![DiscordEmbed {
                title: "New report".into(),
                url: "https://reports.example/reports/123".into(),
                description: "Report details".into(),
                color: 0x666b7a,
                fields: Vec::new(),
                timestamp: "2026-09-15T20:00:00Z".into(),
            }],
            allowed_mentions: DiscordAllowedMentions { parse: Vec::new() },
        }
    }
}
