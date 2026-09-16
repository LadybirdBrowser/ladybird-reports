use askama::Template;
use axum::{
    Extension, Form,
    extract::{Path, Query, State},
    response::Redirect,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{
    domain::{IssueId, ReportId},
    error::{AppError, Result},
};

use super::super::{
    AdminState, TemplateResponse, authentication::Navigation, session::Session,
    templates::not_found,
};
use super::github::{ensure_github_reports_link, refresh_tracked_issue};

#[derive(Deserialize)]
pub struct IssueFilters {
    #[serde(default)]
    resolved: bool,
}

#[derive(Template)]
#[template(path = "issues/index.html")]
pub struct IssuesTemplate {
    navigation: Option<Navigation>,
    include_resolved: bool,
    issues: Vec<IssueRow>,
}

pub struct IssueRow {
    id: IssueId,
    title: String,
    report_count: i64,
    github_number: i64,
    is_resolved: bool,
    github_state: String,
}

#[derive(Template)]
#[template(path = "issues/show.html")]
pub struct IssueTemplate {
    navigation: Option<Navigation>,
    issue: IssueView,
    reports: Vec<ReportView>,
    merge_destinations: Vec<IssueOption>,
    events: Vec<EventView>,
    sync_warning: bool,
    github_field_warning: bool,
}

pub struct IssueView {
    id: IssueId,
    title: String,
    description: String,
    github_number: i64,
    github_url: String,
    is_resolved: bool,
    github_state: String,
    merged_into: Option<IssueId>,
}

pub struct ReportView {
    id: crate::domain::ReportId,
    client_version: String,
    created_at: DateTime<Utc>,
}

pub struct IssueOption {
    id: IssueId,
    title: String,
}

pub struct EventView {
    action: String,
    actor: String,
    details: String,
    created_at: DateTime<Utc>,
}

pub async fn index(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Query(filters): Query<IssueFilters>,
) -> Result<TemplateResponse<IssuesTemplate>> {
    let issues = state
        .database
        .list_issues(filters.resolved)
        .await?
        .into_iter()
        .map(|issue| IssueRow {
            id: issue.id,
            title: issue.title,
            report_count: issue.report_count,
            github_number: issue.github_number,
            is_resolved: issue.resolved_at.is_some(),
            github_state: issue.github_state,
        })
        .collect();

    Ok(TemplateResponse(IssuesTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        include_resolved: filters.resolved,
        issues,
    }))
}

#[derive(Deserialize)]
pub struct CreateIssueForm {
    csrf: String,
    title: String,
    #[serde(default)]
    description: String,
}

pub async fn create_from_report(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
    Form(form): Form<CreateIssueForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;

    crate::infrastructure::database::validate_issue_text(&form.title, &form.description)?;

    let configuration = state.database.configuration().await?;
    let access_token = session.github_access_token(&state)?;
    let github_issue = state
        .github
        .create_issue(
            &access_token,
            &configuration.github_repository,
            form.title.trim(),
            &form.description,
        )
        .await?;
    let assignment = state
        .database
        .assign_report_to_github_issue(
            &github_issue,
            &form.description,
            report_id,
            &configuration.github_repository,
            session.github_id,
        )
        .await
        .map_err(|error| {
            tracing::error!(
                event = "issue.create_tracking_failed",
                github_url = github_issue.html_url,
                ?error,
            );
            error
        })?;

    tracing::info!(
        event = "issue.create",
        issue_id = %assignment.issue_id,
        %report_id,
        github_number = github_issue.number,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/issues/{}", assignment.issue_id)))
}

pub async fn show(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(issue_id): Path<IssueId>,
) -> Result<TemplateResponse<IssueTemplate>> {
    let sync_warning = match refresh_tracked_issue(&state, &session, issue_id).await {
        Ok(_) => false,
        Err(
            AppError::Unavailable
            | AppError::RateLimited
            | AppError::PermissionDenied(_)
            | AppError::Conflict(_),
        ) => {
            tracing::warn!(event = "github.issue_refresh_failed", %issue_id);
            true
        }
        Err(error) => return Err(error),
    };

    let github_field_warning = match ensure_github_reports_link(&state, &session, issue_id).await {
        Ok(()) => false,
        Err(error) => {
            tracing::warn!(event = "github.reports_link_sync_failed", %issue_id, ?error);
            true
        }
    };

    let details = state
        .database
        .issue_details(issue_id)
        .await?
        .ok_or_else(|| not_found("Issue not found"))?;

    let destinations = state
        .database
        .list_issues(false)
        .await?
        .into_iter()
        .filter(|issue| {
            issue.id != issue_id
                && !matches!(
                    issue.github_state.as_str(),
                    "missing" | "moved" | "unavailable"
                )
        })
        .map(|issue| IssueOption {
            id: issue.id,
            title: issue.title,
        })
        .collect();

    let issue = IssueView {
        id: details.issue.id,
        title: details.issue.title,
        description: details.issue.description,
        github_number: details.issue.github_number,
        github_url: details.issue.github_url,
        is_resolved: details.issue.resolved_at.is_some(),
        github_state: details.issue.github_state,
        merged_into: details.issue.merged_into,
    };

    let reports = details
        .reports
        .into_iter()
        .map(|report| ReportView {
            id: report.id,
            client_version: report.client_version,
            created_at: report.created_at,
        })
        .collect();

    let events = details
        .events
        .into_iter()
        .map(|event| EventView {
            action: event.action,
            actor: event.actor_login.unwrap_or_else(|| "system".into()),
            details: event.details.to_string(),
            created_at: event.created_at,
        })
        .collect();

    Ok(TemplateResponse(IssueTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        issue,
        reports,
        merge_destinations: destinations,
        events,
        sync_warning,
        github_field_warning,
    }))
}

#[derive(Deserialize)]
pub struct UpdateIssueForm {
    csrf: String,
    title: String,
    description: String,
}

pub async fn update(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(issue_id): Path<IssueId>,
    Form(form): Form<UpdateIssueForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;

    state
        .database
        .update_issue(issue_id, &form.title, &form.description, session.github_id)
        .await?;

    tracing::info!(
        event = "issue.update",
        %issue_id,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/issues/{issue_id}")))
}

#[derive(Deserialize)]
pub struct ReplaceGithubLinkForm {
    csrf: String,
    github_url: String,
}

pub async fn replace_github_link(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(issue_id): Path<IssueId>,
    Form(form): Form<ReplaceGithubLinkForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    let configuration = state.database.configuration().await?;
    let number = issue_number_from_url(&form.github_url, &configuration.github_repository)?;
    let token = session.github_access_token(&state)?;
    let replacement = state
        .github
        .issue(&token, &configuration.github_repository, number)
        .await?;
    state
        .database
        .replace_github_issue(
            issue_id,
            &configuration.github_repository,
            &replacement,
            session.github_id,
        )
        .await?;

    tracing::info!(event = "issue.update_github_link", %issue_id, github_number = number);
    Ok(Redirect::to(&format!("/issues/{issue_id}")))
}

#[derive(Deserialize)]
pub struct CreateReplacementForm {
    csrf: String,
}

pub async fn create_replacement(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(issue_id): Path<IssueId>,
    Form(form): Form<CreateReplacementForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    let details = state
        .database
        .issue_details(issue_id)
        .await?
        .ok_or(AppError::NotFound("Issue not found"))?;
    if !matches!(
        details.issue.github_state.as_str(),
        "missing" | "moved" | "unavailable"
    ) {
        return Err(AppError::Conflict(
            "Current GitHub issue is still available",
        ));
    }

    let configuration = state.database.configuration().await?;
    let token = session.github_access_token(&state)?;
    let replacement = state
        .github
        .create_issue(
            &token,
            &configuration.github_repository,
            &details.issue.title,
            &details.issue.description,
        )
        .await?;
    state
        .database
        .replace_github_issue(
            issue_id,
            &configuration.github_repository,
            &replacement,
            session.github_id,
        )
        .await
        .map_err(|error| {
            tracing::error!(
                event = "issue.replace_tracking_failed",
                github_url = replacement.html_url,
                ?error,
            );
            error
        })?;

    tracing::info!(event = "issue.update_github_link", %issue_id, github_number = replacement.number);
    Ok(Redirect::to(&format!("/issues/{issue_id}")))
}

fn issue_number_from_url(url: &str, repository: &str) -> Result<i64> {
    let url = reqwest::Url::parse(url)
        .map_err(|_| AppError::InvalidRequest("Enter a GitHub issue URL"))?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err(AppError::InvalidRequest("Enter a GitHub issue URL"));
    }

    let segments = url
        .path_segments()
        .ok_or(AppError::InvalidRequest("Enter a GitHub issue URL"))?
        .collect::<Vec<_>>();
    if segments.len() != 4
        || !format!("{}/{}", segments[0], segments[1]).eq_ignore_ascii_case(repository)
        || segments[2] != "issues"
    {
        return Err(AppError::InvalidRequest(
            "Issue is not in the configured repository",
        ));
    }

    let number = segments[3]
        .parse::<i64>()
        .map_err(|_| AppError::InvalidRequest("Enter a GitHub issue URL"))?;
    if number < 1 {
        return Err(AppError::InvalidRequest("Enter a GitHub issue URL"));
    }
    Ok(number)
}

#[derive(Deserialize)]
pub struct MergeIssueForm {
    csrf: String,
    destination: IssueId,
}

pub async fn merge(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(issue_id): Path<IssueId>,
    Form(form): Form<MergeIssueForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;

    if issue_id == form.destination {
        return Err(AppError::InvalidRequest(
            "An issue cannot be merged into itself",
        ));
    }

    let source = refresh_tracked_issue(&state, &session, issue_id).await?;
    let destination = refresh_tracked_issue(&state, &session, form.destination).await?;
    if destination.github_state != "open" || destination.merged_into.is_some() {
        return Err(AppError::Conflict("Destination GitHub issue is not open"));
    }
    if source.merged_into.is_some() {
        return Err(AppError::Conflict("Source issue was already merged"));
    }

    state
        .database
        .merge_issue(issue_id, form.destination, session.github_id)
        .await?;

    tracing::info!(
        event = "issue.merge",
        source_issue_id = %issue_id,
        destination_issue_id = %form.destination,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/issues/{}", form.destination)))
}
