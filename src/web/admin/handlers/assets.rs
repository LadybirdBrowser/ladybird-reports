use std::sync::LazyLock;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::domain::sha256_hex;
use crate::error::Result;
use sha2::{Digest, Sha256};

use super::super::AdminState;

const STYLESHEET: &str = include_str!("../../../../assets/application.css");
const JAVASCRIPT: &str = include_str!("../../../../assets/application.js");
const REPORTS_JAVASCRIPT: &str = include_str!("../../../../assets/reports.js");
const LIST_SEARCH_JAVASCRIPT: &str = include_str!("../../../../assets/list-search.js");
const GITHUB_ICON: &str = include_str!("../../../../assets/github-icon.svg");
const LADYBIRD_MARK: &[u8] = include_bytes!("../../../../assets/ladybird-mark.png");

static STYLESHEET_ETAG: LazyLock<HeaderValue> = LazyLock::new(|| asset_etag(STYLESHEET));
static JAVASCRIPT_ETAG: LazyLock<HeaderValue> = LazyLock::new(|| asset_etag(JAVASCRIPT));
static REPORTS_JAVASCRIPT_ETAG: LazyLock<HeaderValue> =
    LazyLock::new(|| asset_etag(REPORTS_JAVASCRIPT));
static LIST_SEARCH_JAVASCRIPT_ETAG: LazyLock<HeaderValue> =
    LazyLock::new(|| asset_etag(LIST_SEARCH_JAVASCRIPT));
static GITHUB_ICON_ETAG: LazyLock<HeaderValue> = LazyLock::new(|| asset_etag(GITHUB_ICON));
static LADYBIRD_MARK_ETAG: LazyLock<HeaderValue> = LazyLock::new(|| asset_etag(LADYBIRD_MARK));
static ASSET_VERSION: LazyLock<String> = LazyLock::new(|| {
    let mut digest = Sha256::new();
    for asset in [
        STYLESHEET.as_bytes(),
        JAVASCRIPT.as_bytes(),
        REPORTS_JAVASCRIPT.as_bytes(),
        LIST_SEARCH_JAVASCRIPT.as_bytes(),
        GITHUB_ICON.as_bytes(),
        LADYBIRD_MARK,
    ] {
        digest.update((asset.len() as u64).to_be_bytes());
        digest.update(asset);
    }
    hex::encode(digest.finalize())
});

pub fn asset_version() -> &'static str {
    ASSET_VERSION.as_str()
}

pub async fn versioned_asset(Path((version, name)): Path<(String, String)>) -> Response {
    if version != asset_version() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let Some((body, content_type, etag)) = asset(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut response = ([(header::CONTENT_TYPE, content_type)], body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response.headers_mut().insert(header::ETAG, etag.clone());
    response
}

pub async fn unversioned_asset(Path(name): Path<String>, headers: HeaderMap) -> Response {
    let Some((body, content_type, etag)) = asset(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    static_asset_bytes(&headers, body, content_type, etag)
}

fn asset(name: &str) -> Option<(&'static [u8], &'static str, &'static HeaderValue)> {
    Some(match name {
        "application.css" => (
            STYLESHEET.as_bytes(),
            "text/css; charset=utf-8",
            &STYLESHEET_ETAG,
        ),
        "application.js" => (
            JAVASCRIPT.as_bytes(),
            "text/javascript; charset=utf-8",
            &JAVASCRIPT_ETAG,
        ),
        "reports.js" => (
            REPORTS_JAVASCRIPT.as_bytes(),
            "text/javascript; charset=utf-8",
            &REPORTS_JAVASCRIPT_ETAG,
        ),
        "list-search.js" => (
            LIST_SEARCH_JAVASCRIPT.as_bytes(),
            "text/javascript; charset=utf-8",
            &LIST_SEARCH_JAVASCRIPT_ETAG,
        ),
        "github-icon.svg" => (GITHUB_ICON.as_bytes(), "image/svg+xml", &GITHUB_ICON_ETAG),
        "ladybird-mark.png" => (LADYBIRD_MARK, "image/png", &LADYBIRD_MARK_ETAG),
        _ => return None,
    })
}

fn static_asset_bytes(
    request_headers: &HeaderMap,
    body: &'static [u8],
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

fn asset_etag(body: impl AsRef<[u8]>) -> HeaderValue {
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
