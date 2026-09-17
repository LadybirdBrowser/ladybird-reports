use askama::Template;
use axum::{
    http::{Method, StatusCode, Uri},
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
