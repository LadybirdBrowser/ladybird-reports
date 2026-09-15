use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("invalid request: {0}")]
    InvalidRequest(&'static str),

    #[error("authentication required")]
    AuthenticationRequired,

    #[error("permission denied: {0}")]
    PermissionDenied(&'static str),

    #[error("resource not found: {0}")]
    NotFound(&'static str),

    #[error("request conflicts with existing state: {0}")]
    Conflict(&'static str),

    #[error("rate limit exceeded")]
    RateLimited,

    #[error("payload is too large")]
    PayloadTooLarge,

    #[error("request deadline exceeded")]
    DeadlineExceeded,

    #[error("service is temporarily unavailable")]
    Unavailable,

    #[error(transparent)]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            Self::AuthenticationRequired => StatusCode::UNAUTHORIZED,
            Self::PermissionDenied(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::DeadlineExceeded => StatusCode::REQUEST_TIMEOUT,
            Self::Unavailable | Self::Database(_) | Self::Io(_) | Self::Internal(_) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
        }
    }

    pub fn public_message(&self) -> &'static str {
        match self {
            Self::InvalidRequest(message)
            | Self::PermissionDenied(message)
            | Self::NotFound(message)
            | Self::Conflict(message) => message,
            Self::AuthenticationRequired => "Authentication required",
            Self::RateLimited => "Rate limit exceeded",
            Self::PayloadTooLarge => "Payload is too large",
            Self::DeadlineExceeded => "Request deadline exceeded",
            Self::Unavailable | Self::Database(_) | Self::Io(_) | Self::Internal(_) => {
                "Service temporarily unavailable"
            }
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();

        if matches!(self, Self::Database(_) | Self::Io(_) | Self::Internal(_)) {
            tracing::error!(error = ?self, "Request failed because of an internal error");
        }

        let mut response =
            (status, Json(json!({ "error": self.public_message() }))).into_response();

        if status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert("retry-after", "60".parse().expect("static header value"));
        }

        response
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
