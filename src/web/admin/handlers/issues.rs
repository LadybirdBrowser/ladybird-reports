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
    github_number: Option<i64>,
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
    github_number: Option<i64>,
    github_url: Option<String>,
    github_publish_state: Option<String>,
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
    github_action: String,
    #[serde(default)]
    github_issue_number: String,
    #[serde(default)]
    github_title: String,
    #[serde(default)]
    github_body: String,
}

pub async fn create_from_report(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
    Form(form): Form<CreateIssueForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;

    let configuration = state.database.configuration().await?;
    let access_token = match form.github_action.as_str() {
        "link" | "create" => Some(session.github_access_token(&state)?),
        "none" => None,
        _ => return Err(AppError::InvalidRequest("Invalid GitHub action")),
    };

    let existing_github_issue = match form.github_action.as_str() {
        "none" | "create" => None,
        "link" => {
            let number = form
                .github_issue_number
                .parse::<i64>()
                .map_err(|_| AppError::InvalidRequest("Select a GitHub issue"))?;
            if number < 1 {
                return Err(AppError::InvalidRequest("Select a GitHub issue"));
            }

            Some(
                state
                    .github
                    .issue(
                        access_token.as_deref().expect("link action has a token"),
                        &configuration.github_repository,
                        number,
                    )
                    .await?,
            )
        }
        _ => return Err(AppError::InvalidRequest("Invalid GitHub action")),
    };

    if form.github_action == "create"
        && (form.github_title.trim().is_empty()
            || form.github_title.len() > 256
            || form.github_body.len() > 256 * 1024)
    {
        return Err(AppError::InvalidRequest("Invalid GitHub issue contents"));
    }

    if let Some(github_issue) = existing_github_issue {
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
            event = if assignment.created {
                "issue.created"
            } else {
                "report.assigned_to_linked_github_issue"
            },
            issue_id = %assignment.issue_id,
            %report_id,
            github_number = github_issue.number,
            actor = session.login,
        );

        return Ok(Redirect::to(&format!("/issues/{}", assignment.issue_id)));
    }

    let issue_id = state
        .database
        .create_issue(&form.title, &form.description, report_id, session.github_id)
        .await?;

    if form.github_action == "create" {
        let attempt_id = state
            .database
            .begin_github_publish(issue_id, session.github_id)
            .await?;
        let github_issue = match state
            .github
            .create_issue(
                access_token.as_deref().expect("create action has a token"),
                &configuration.github_repository,
                form.github_title.trim(),
                &form.github_body,
            )
            .await
        {
            Ok(issue) => issue,
            Err(error) => {
                if matches!(error, AppError::Internal(_) | AppError::Unavailable) {
                    state
                        .database
                        .mark_github_publish_uncertain(attempt_id)
                        .await?;
                } else {
                    state
                        .database
                        .mark_github_publish_failed(attempt_id)
                        .await?;
                }
                return Err(error);
            }
        };

        state
            .database
            .finish_github_publish(
                attempt_id,
                issue_id,
                github_issue.number,
                &github_issue.html_url,
                session.github_id,
            )
            .await?;
    }

    tracing::info!(
        event = "issue.created",
        %issue_id,
        %report_id,
        github_action = form.github_action,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/issues/{issue_id}")))
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
        github_publish_state: details.issue.github_publish_state,
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
