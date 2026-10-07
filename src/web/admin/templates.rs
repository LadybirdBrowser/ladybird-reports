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

/// The timestamp format used throughout the admin UI.
pub fn display_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%d %b %Y, %H:%M UTC").to_string()
}

/// An audit event as listed in the activity section of a report or issue page.
pub struct HistoryEvent {
    pub action: String,
    pub actor: String,
    pub details: String,
    pub created_at: DateTime<Utc>,
}

impl From<AuditEvent> for HistoryEvent {
    fn from(event: AuditEvent) -> Self {
        Self {
            action: event.action,
            actor: event.actor_login.unwrap_or_else(|| "system".into()),
            details: event.details.to_string(),
            created_at: event.created_at,
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
