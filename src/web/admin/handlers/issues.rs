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
    error::Result,
};

use super::super::{
    AdminState, TemplateResponse, authentication::Navigation, session::Session,
    templates::not_found,
};
use super::github::github_body;

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
}

#[derive(Template)]
#[template(path = "issues/show.html")]
pub struct IssueTemplate {
    navigation: Option<Navigation>,
    issue: IssueView,
    reports: Vec<ReportView>,
    merge_destinations: Vec<IssueOption>,
    events: Vec<EventView>,
}

pub struct IssueView {
    id: IssueId,
    title: String,
    description: String,
    github_number: i64,
    github_url: String,
    is_resolved: bool,
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
            &github_body(&form.description, 1),
        )
        .await?;
    let assignment = state
        .database
        .assign_report_to_github_issue(
            &form.title,
            &form.description,
            report_id,
            github_issue.number,
            &github_issue.html_url,
            session.github_id,
        )
        .await?;

    tracing::info!(
        event = "issue.created",
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
        .filter(|issue| issue.id != issue_id)
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
    }))
}

#[derive(Deserialize)]
pub struct UpdateIssueForm {
    csrf: String,
    title: String,
    description: String,
    #[serde(default)]
    resolved: bool,
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
        .update_issue(
            issue_id,
            &form.title,
            &form.description,
            form.resolved,
            session.github_id,
        )
        .await?;

    Ok(Redirect::to(&format!("/issues/{issue_id}")))
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

    state
        .database
        .merge_issue(issue_id, form.destination, session.github_id)
        .await?;

    tracing::info!(
        event = "issue.merged",
        source_issue_id = %issue_id,
        destination_issue_id = %form.destination,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/issues/{}", form.destination)))
}
