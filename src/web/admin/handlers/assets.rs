use std::sync::LazyLock;

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::domain::sha256_hex;
use crate::error::Result;

use super::super::AdminState;

const STYLESHEET: &str = include_str!("../../../../assets/application.css");
const JAVASCRIPT: &str = include_str!("../../../../assets/application.js");

static STYLESHEET_ETAG: LazyLock<HeaderValue> = LazyLock::new(|| asset_etag(STYLESHEET));
static JAVASCRIPT_ETAG: LazyLock<HeaderValue> = LazyLock::new(|| asset_etag(JAVASCRIPT));

pub async fn stylesheet(headers: HeaderMap) -> Response {
    static_asset(
        &headers,
        STYLESHEET,
        "text/css; charset=utf-8",
        &STYLESHEET_ETAG,
    )
}

pub async fn javascript(headers: HeaderMap) -> Response {
    static_asset(
        &headers,
        JAVASCRIPT,
        "text/javascript; charset=utf-8",
        &JAVASCRIPT_ETAG,
    )
}

fn static_asset(
    request_headers: &HeaderMap,
    body: &'static str,
    content_type: &'static str,
    etag: &HeaderValue,
) -> Response {
    let is_current = request_headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|candidates| etag_matches(candidates, etag));

    let mut response = if is_current {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, content_type)], body).into_response()
    };

    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=0, must-revalidate"),
    );
    response.headers_mut().insert(header::ETAG, etag.clone());

    response
}

fn asset_etag(body: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("\"{}\"", sha256_hex(body)))
        .expect("a SHA-256 digest is a valid ETag")
}

fn etag_matches(candidates: &HeaderValue, current: &HeaderValue) -> bool {
    let Ok(candidates) = candidates.to_str() else {
        return false;
    };
    let current = current.to_str().expect("generated ETag is ASCII");

    candidates
        .split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == current)
}

pub async fn live() -> &'static str {
    "ok"
}

pub async fn ready(State(state): State<AdminState>) -> Result<&'static str> {
    state.database.healthcheck().await?;
    tokio::fs::metadata(state.attachments.root()).await?;
    Ok("ok")
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::etag_matches;

    #[test]
    fn matches_etags_in_conditional_request_lists() {
        let current = HeaderValue::from_static("\"current\"");

        assert!(etag_matches(
            &HeaderValue::from_static("\"old\", W/\"current\""),
            &current,
        ));
        assert!(etag_matches(&HeaderValue::from_static("*"), &current));
        assert!(!etag_matches(
            &HeaderValue::from_static("\"old\""),
            &current,
        ));
    }
}
