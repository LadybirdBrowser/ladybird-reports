//! Links that share a report without signing in. A maintainer creates them from the report page; the
//! routes that open one are the only ones of the admin service that need no session, so they do as little as they can:
//! look the token up, and answer with a document or an attachment of the report it belongs to.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    Extension, Form, Json,
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::{
    application::report_share_document,
    domain::{AttachmentId, ReportId},
    error::{AppError, Result},
    infrastructure::{hash_secret, random_token},
};

use super::super::{
    AdminState,
    session::{CsrfForm, Session},
};
use super::reports::attachment_response;

#[derive(Serialize)]
pub struct CreatedShare {
    url: String,
}

/// Creates a link to the report. The link is in the answer and nowhere else: only a hash of its token is kept.
pub async fn create(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
    Form(form): Form<CsrfForm>,
) -> Result<Json<CreatedShare>> {
    session.verify_csrf(&form.csrf)?;

    let token = random_token();
    state
        .database
        .create_report_share(report_id, session.github_id, &hash_secret(&token))
        .await?;
    let configuration = state.database.configuration().await?;

    tracing::info!(
        event = "report.share_create",
        %report_id,
        actor = session.login,
    );

    Ok(Json(CreatedShare {
        url: share_url(&configuration.admin_base_url, &token),
    }))
}

/// The report of a link, as YAML.
pub async fn shared_report(
    State(state): State<AdminState>,
    Path(token): Path<String>,
) -> Result<Response> {
    let (access, report_id) = open(&state, &token).await?;
    let details = state
        .database
        .report_details(report_id)
        .await?
        .ok_or_else(not_found)?;
    let issue = match details.report.issue_id {
        Some(issue_id) => state.database.find_issue(issue_id).await?,
        None => None,
    };
    let configuration = state.database.configuration().await?;

    let document = report_share_document(
        &details,
        issue.as_ref(),
        &share_url(&configuration.admin_base_url, &token),
        access,
    )?;

    // Not compressed: the document holds the address of the link next to text that the reporter chose.
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        document,
    )
        .into_response())
}

/// An attachment of the report of a link.
pub async fn shared_attachment(
    State(state): State<AdminState>,
    Path((token, attachment_id)): Path<(String, AttachmentId)>,
) -> Result<Response> {
    let (_, report_id) = open(&state, &token).await?;
    let details = state
        .database
        .report_details(report_id)
        .await?
        .ok_or_else(not_found)?;
    let attachment = details
        .attachments
        .iter()
        .find(|attachment| attachment.id == attachment_id)
        .ok_or_else(not_found)?;

    attachment_response(&state, attachment, true, false).await
}

/// Looks a token up and counts the use. Whatever is wrong with the link, the answer is the same.
async fn open(
    state: &AdminState,
    token: &str,
) -> Result<(chrono::DateTime<chrono::Utc>, ReportId)> {
    if !within_request_budget() {
        return Err(AppError::RateLimited);
    }
    if token.len() > 64
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(not_found());
    }

    let access = state
        .database
        .open_report_share(&hash_secret(token))
        .await?
        .ok_or_else(not_found)?;

    Ok((access.expires_at, access.report_id))
}

fn not_found() -> AppError {
    AppError::NotFound("Not found")
}

fn share_url(admin_base_url: &str, token: &str) -> String {
    format!("{}/share/{token}", admin_base_url.trim_end_matches('/'))
}

/// A limit on all requests for links together, so that a flood of guesses cannot weigh on the service. The admin
/// service does not know the address a request came from, and a token cannot be guessed, so this only bounds the load.
fn within_request_budget() -> bool {
    const REQUESTS_PER_MINUTE: u32 = 300;
    static WINDOW: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

    let mut window = WINDOW.lock().unwrap_or_else(|error| error.into_inner());
    let now = Instant::now();
    let (started, count) = window.get_or_insert((now, 0));
    if now.duration_since(*started) >= Duration::from_secs(60) {
        *started = now;
        *count = 0;
    }
    *count += 1;

    *count <= REQUESTS_PER_MINUTE
}
