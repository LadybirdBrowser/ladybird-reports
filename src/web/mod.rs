pub mod admin;
pub mod public;

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};
use tower_http::{
    request_id::RequestId,
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::Level;

pub fn with_observability<S>(router: axum::Router<S>) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(
        tower::ServiceBuilder::new()
            .layer(axum::middleware::from_fn(assign_request_id))
            .layer(
                TraceLayer::new_for_http()
                    .make_span_with(|request: &Request| {
                        let request_id = request
                            .headers()
                            .get("x-request-id")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("invalid");

                        tracing::info_span!(
                            "http.request",
                            method = %request.method(),
                            path = request.uri().path(),
                            request_id,
                        )
                    })
                    .on_request(())
                    .on_response(
                        DefaultOnResponse::new()
                            .level(Level::INFO)
                            .include_headers(false),
                    ),
            ),
    )
}

async fn assign_request_id(mut request: Request, next: Next) -> Response {
    let request_id = HeaderValue::from_str(&uuid::Uuid::now_v7().to_string())
        .expect("a UUID is a valid header value");

    request
        .headers_mut()
        .insert("x-request-id", request_id.clone());
    request
        .extensions_mut()
        .insert(RequestId::new(request_id.clone()));

    let mut response = next.run(request).await;
    response.headers_mut().insert("x-request-id", request_id);
    response
}

pub async fn admin_response_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    add_security_headers(response.headers_mut());

    response
        .headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("private, no-store"));

    response
}

pub async fn public_response_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    add_security_headers(response.headers_mut());

    response
        .headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));

    response
}

fn add_security_headers(headers: &mut HeaderMap) {
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'none'; style-src 'self'; script-src 'self'; img-src 'self'; \
             form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("accelerometer=(), camera=(), geolocation=(), microphone=()"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("strict-transport-security"),
        HeaderValue::from_static("max-age=31536000"),
    );
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, http::Request, middleware, routing::get};
    use tower::ServiceExt;

    use super::{admin_response_headers, public_response_headers};

    #[tokio::test]
    async fn admin_responses_are_private_and_not_stored() {
        let response = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(middleware::from_fn(admin_response_headers))
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("cache-control").unwrap(),
            "private, no-store"
        );
        assert_security_headers(response.headers());
    }

    #[tokio::test]
    async fn public_responses_are_not_stored() {
        let response = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(middleware::from_fn(public_response_headers))
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        assert_security_headers(response.headers());
    }

    fn assert_security_headers(headers: &axum::http::HeaderMap) {
        assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(headers.get("referrer-policy").unwrap(), "no-referrer");
        assert_eq!(
            headers.get("cross-origin-opener-policy").unwrap(),
            "same-origin"
        );
        assert_eq!(
            headers.get("cross-origin-resource-policy").unwrap(),
            "same-origin"
        );
        assert!(
            headers
                .get("content-security-policy")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("default-src 'none'")
        );
    }
}
