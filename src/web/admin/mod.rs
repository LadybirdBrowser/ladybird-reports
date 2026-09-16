mod authentication;
mod handlers;
mod session;
mod templates;

use std::sync::Arc;

use axum::{
    Router, middleware,
    routing::{get, post},
};

use crate::infrastructure::{
    SecretCipher, attachments::FileAttachmentStore, database::AdminDatabase, github::GithubClient,
};

pub use templates::TemplateResponse;

#[derive(Clone)]
pub struct AdminState {
    pub database: AdminDatabase,
    pub attachments: FileAttachmentStore,
    pub github: GithubClient,
    pub secret_cipher: SecretCipher,
    pub bootstrap_reporting_database_url: Option<Arc<str>>,
}

pub fn router(state: AdminState) -> Router {
    let authenticated = Router::new()
        .route("/", get(handlers::reports::index))
        .route("/reports/{id}", get(handlers::reports::show))
        .route(
            "/api/report-options",
            get(handlers::reports::search_options),
        )
        .route("/api/report-list", get(handlers::reports::list))
        .route(
            "/api/report-search-completions",
            get(handlers::reports::search_completions),
        )
        .route("/api/issue-options", get(handlers::github::issue_options))
        .route("/reports/{id}/block-ip", post(handlers::reports::block_ip))
        .route(
            "/reports/{id}/unblock-ip",
            post(handlers::reports::unblock_ip),
        )
        .route(
            "/reports/{report_id}/issue",
            post(handlers::reports::assign_to_issue),
        )
        .route(
            "/reports/{report_id}/state",
            post(handlers::reports::set_state),
        )
        .route("/reports/{report_id}/hide", post(handlers::reports::hide))
        .route("/attachments/{id}", get(handlers::reports::attachment))
        .route("/issues", get(handlers::issues::index))
        .route(
            "/reports/{id}/issues",
            post(handlers::issues::create_from_report),
        )
        .route("/issues/{id}", get(handlers::issues::show))
        .route("/issues/{id}", post(handlers::issues::update))
        .route("/issues/{id}/merge", post(handlers::issues::merge))
        .route("/settings", get(handlers::settings::show))
        .route("/settings", post(handlers::settings::update))
        .route("/settings/fields", post(handlers::settings::update_field))
        .route(
            "/settings/fields/order",
            post(handlers::settings::reorder_fields),
        )
        .route("/operations", get(handlers::settings::operations))
        .route("/logout", post(authentication::logout))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            session::require_session,
        ));

    let routes = Router::new()
        .merge(authenticated)
        .route("/login", get(authentication::login))
        .route("/auth/github", get(authentication::start_github_login))
        .route("/auth/callback", get(authentication::github_callback))
        .route("/assets/application.css", get(handlers::assets::stylesheet))
        .route("/assets/application.js", get(handlers::assets::javascript))
        .route(
            "/assets/reports.js",
            get(handlers::assets::reports_javascript),
        )
        .route(
            "/assets/github-icon.svg",
            get(handlers::assets::github_icon),
        )
        .route(
            "/assets/ladybird-mark.png",
            get(handlers::assets::ladybird_mark),
        )
        .route("/health/live", get(handlers::assets::live))
        .route("/health/ready", get(handlers::assets::ready))
        .layer(middleware::from_fn(super::admin_response_headers));

    super::with_observability(routes).with_state(state)
}
