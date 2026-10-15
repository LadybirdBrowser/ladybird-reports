use askama::Template;
use axum::{
    http::{Method, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
};
use chrono::{DateTime, Utc};

use crate::{error::AppError, infrastructure::database::AuditEvent};

pub struct TemplateResponse<T>(pub T);

impl<T> IntoResponse for TemplateResponse<T>
where
    T: Template,
{
    fn into_response(self) -> Response {
        match self.0.render() {
            Ok(html) => Html(html).into_response(),
            Err(error) => {
                tracing::error!(?error, "Could not render management template");
                AppError::Unavailable.into_response()
            }
        }
    }
}

/// A point in time, rendered as a `<time>` element. The text is the full timestamp; `application.js` swaps it for a
/// relative one while it is recent.
#[derive(Clone, Copy)]
pub struct Timestamp(pub DateTime<Utc>);

impl Timestamp {
    /// The full timestamp as plain text.
    pub fn text(self) -> String {
        self.0.format("%d %b %Y, %H:%M UTC").to_string()
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            r#"<time datetime="{}">{}</time>"#,
            self.0.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            self.text()
        )
    }
}

impl askama::filters::HtmlSafe for Timestamp {}

pub fn display_timestamp(timestamp: DateTime<Utc>) -> Timestamp {
    Timestamp(timestamp)
}

/// An audit event as listed in the activity section of a report or issue page.
pub struct HistoryEvent {
    pub action: String,
    pub actor: String,
    pub details: String,
    pub created_at: Timestamp,
}

impl From<AuditEvent> for HistoryEvent {
    fn from(event: AuditEvent) -> Self {
        Self {
            action: event.action,
            actor: event.actor_login.unwrap_or_else(|| "system".into()),
            details: event.details.to_string(),
            created_at: Timestamp(event.created_at),
        }
    }
}

pub fn redirect_to_login(method: &Method, uri: &Uri) -> Response {
    let location = if method == Method::GET {
        uri.path_and_query()
            .and_then(|path| super::authentication::validated_return_to(path.as_str()))
            .filter(|path| *path != "/")
            .map(|path| super::authentication::url_with_next("/login", path))
            .unwrap_or_else(|| "/login".to_owned())
    } else {
        "/login".to_owned()
    };

    (
        StatusCode::SEE_OTHER,
        [("location", location)],
        "Authentication required",
    )
        .into_response()
}
