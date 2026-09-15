use askama::Template;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use crate::error::AppError;

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

pub fn not_found(message: &'static str) -> AppError {
    AppError::NotFound(message)
}

pub fn redirect_to_login() -> Response {
    (
        StatusCode::SEE_OTHER,
        [("location", "/login")],
        "Authentication required",
    )
        .into_response()
}
