mod handlers;
mod rate_limit;

use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
};

use crate::{
    application::ReportIngestionService, domain::HARD_MAX_BODY_BYTES,
    infrastructure::database::IngestDatabase,
};

#[derive(Clone)]
pub struct PublicState {
    pub ingestion: ReportIngestionService,
    pub database: IngestDatabase,
    pub client_address_key: Arc<[u8; 32]>,
}

pub fn router(state: PublicState) -> Router {
    let routes = Router::new()
        .route("/api/v1/challenges", post(handlers::issue_challenge))
        .route(
            "/api/v1/reports",
            post(handlers::submit_report).layer(DefaultBodyLimit::max(HARD_MAX_BODY_BYTES)),
        )
        .route("/health/live", get(handlers::live))
        .route("/health/ready", get(handlers::ready))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::limit_public_request,
        ));

    let routes = routes.layer(middleware::from_fn(super::public_response_headers));

    super::with_observability(routes).with_state(state)
}
