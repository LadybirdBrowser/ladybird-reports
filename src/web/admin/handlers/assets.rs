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

struct AssetSource {
    name: &'static str,
    content_type: &'static str,
    body: &'static [u8],
}

struct Asset {
    name: &'static str,
    content_type: &'static str,
    body: &'static [u8],
    etag: HeaderValue,
}

/// Every embedded asset. Adding one here is all that is needed to serve it and
/// to include it in the content-derived asset version.
const ASSET_SOURCES: [AssetSource; 7] = [
    AssetSource {
        name: "application.css",
        content_type: "text/css; charset=utf-8",
        body: include_bytes!("../../../../assets/application.css"),
    },
    AssetSource {
        name: "application.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_bytes!("../../../../assets/application.js"),
    },
    AssetSource {
        name: "reports.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_bytes!("../../../../assets/reports.js"),
    },
    AssetSource {
        name: "audit-log.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_bytes!("../../../../assets/audit-log.js"),
    },
    AssetSource {
        name: "list-search.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_bytes!("../../../../assets/list-search.js"),
    },
    AssetSource {
        name: "github-icon.svg",
        content_type: "image/svg+xml",
        body: include_bytes!("../../../../assets/github-icon.svg"),
    },
    AssetSource {
        name: "ladybird-mark.png",
        content_type: "image/png",
        body: include_bytes!("../../../../assets/ladybird-mark.png"),
    },
];

static ASSETS: LazyLock<Vec<Asset>> = LazyLock::new(|| {
    ASSET_SOURCES
        .iter()
        .map(|source| Asset {
            name: source.name,
            content_type: source.content_type,
            body: source.body,
            etag: asset_etag(source.body),
        })
        .collect()
});

static ASSET_VERSION: LazyLock<String> = LazyLock::new(|| {
    let mut digest = Sha256::new();
    for asset in ASSETS.iter() {
        digest.update((asset.body.len() as u64).to_be_bytes());
        digest.update(asset.body);
    }
    hex::encode(digest.finalize())
});

pub fn asset_version() -> &'static str {
    ASSET_VERSION.as_str()
}

fn asset(name: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|asset| asset.name == name)
}

fn with_caching(
    mut response: Response,
    cache_control: &'static str,
    etag: &HeaderValue,
) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    headers.insert(header::ETAG, etag.clone());
    response
}

pub async fn versioned_asset(Path((version, name)): Path<(String, String)>) -> Response {
    if version != asset_version() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let Some(asset) = asset(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    with_caching(
        ([(header::CONTENT_TYPE, asset.content_type)], asset.body).into_response(),
        "public, max-age=31536000, immutable",
        &asset.etag,
    )
}

pub async fn unversioned_asset(Path(name): Path<String>, headers: HeaderMap) -> Response {
    let Some(asset) = asset(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let is_current = headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|candidates| etag_matches(candidates, &asset.etag));
    let response = if is_current {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, asset.content_type)], asset.body).into_response()
    };

    with_caching(response, "public, max-age=0, must-revalidate", &asset.etag)
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
