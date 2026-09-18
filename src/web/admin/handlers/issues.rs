use askama::Template;
use axum::{
    Extension, Form, Json,
    extract::{Path, Query, State},
    response::Redirect,
};
use chrono::{DateTime, Utc};
use comrak::{Options, markdown_to_html};
use serde::{Deserialize, Serialize};

use crate::{
    domain::{IssueId, IssueSearch, ReportId},
    error::{AppError, Result},
};

use super::super::{
    AdminState, TemplateResponse, authentication::Navigation, session::Session,
    templates::not_found,
};
use super::github::ensure_github_reports_link;

#[derive(Deserialize)]
pub struct IssueFilters {
    #[serde(default = "default_issue_search")]
    q: String,
}

#[derive(Template)]
#[template(path = "issues/index.html")]
pub struct IssuesTemplate {
    navigation: Option<Navigation>,
    asset_version: &'static str,
    search: String,
    issues: Vec<IssueRow>,
}

#[derive(Template)]
#[template(path = "issues/_list.html")]
pub struct IssueListTemplate {
    issues: Vec<IssueRow>,
}

#[derive(Deserialize)]
pub struct IssueCompletionQuery {
    #[serde(default)]
    token: String,
}

#[derive(Serialize)]
pub struct IssueCompletionResponse {
    results: Vec<IssueCompletion>,
}

#[derive(Serialize)]
pub struct IssueCompletion {
    replacement: String,
    label: String,
    description: String,
}

pub struct IssueRow {
    id: IssueId,
    title: String,
    report_count: i64,
    github_number: i64,
    state: String,
}

#[derive(Template)]
#[template(path = "issues/show.html")]
pub struct IssueTemplate {
    navigation: Option<Navigation>,
    asset_version: &'static str,
    issue: IssueView,
    reports: Vec<ReportView>,
    potential_matches: Vec<ReportView>,
    events: Vec<EventView>,
    github_field_warning: bool,
}

pub struct IssueView {
    id: IssueId,
    title: String,
    description: String,
    description_html: String,
    github_number: i64,
    github_url: String,
    state: String,
    github_state: String,
    merged_into: Option<IssueId>,
}

pub struct ReportView {
    id: crate::domain::ReportId,
    title: String,
    client_version: String,
    created_at: DateTime<Utc>,
}

pub struct EventView {
    action: String,
    actor: String,
    details: String,
    created_at: DateTime<Utc>,
}

fn render_issue_description(markdown: &str) -> String {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.alerts = true;
    options.render.escape = true;
    options.render.hardbreaks = true;

    // GitHub issue bodies can include HTML. Leave it escaped because the
    // description is supplied by another service and inserted into our UI.
    markdown_to_html(markdown, &options)
}

#[cfg(test)]
mod markdown_tests {
    use super::render_issue_description;

    #[test]
    fn renders_github_markdown_without_trusting_raw_html() {
        let html = render_issue_description(
            "| Field | Value |\n| --- | --- |\n| Stack | `frame` |\n\n- [x] Checked\n\n<script>alert(1)</script>",
        );

        assert!(html.contains("<table>"));
        assert!(html.contains("<code>frame</code>"));
        assert!(html.contains("type=\"checkbox\""));
        assert!(html.contains("&lt;script&gt;"));
        assert!(
            !render_issue_description("[bad](javascript:alert(1))").contains("href=\"javascript:")
        );
    }

    #[test]
    fn preserves_line_breaks_between_links_in_github_issue_bodies() {
        let html = render_issue_description(
            "Here are some sample files:\n\nhttps://example.com/first\nhttps://example.com/second\n\nTrace:\n```\nframe one\nframe two\n```",
        );

        assert!(html.contains(concat!(
            "<a href=\"https://example.com/first\">https://example.com/first</a>",
            "<br />\n",
            "<a href=\"https://example.com/second\">",
        )));
        assert!(html.contains("frame one\nframe two"));
    }
}

pub async fn index(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Query(filters): Query<IssueFilters>,
) -> Result<TemplateResponse<IssuesTemplate>> {
    let issues = load_issue_list(&state, &filters).await?.issues;

    Ok(TemplateResponse(IssuesTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        asset_version: super::assets::asset_version(),
        search: filters.q,
        issues,
    }))
}

pub async fn list(
    State(state): State<AdminState>,
    Query(filters): Query<IssueFilters>,
) -> Result<TemplateResponse<IssueListTemplate>> {
    Ok(TemplateResponse(load_issue_list(&state, &filters).await?))
}

async fn load_issue_list(state: &AdminState, filters: &IssueFilters) -> Result<IssueListTemplate> {
    let issues = state
        .database
        .list_issues(&IssueSearch::parse(&filters.q)?)
        .await?
        .into_iter()
        .map(|issue| IssueRow {
            id: issue.id,
            title: issue.title,
            report_count: issue.report_count,
            github_number: issue.github_number,
            state: issue.state,
        })
        .collect();

    Ok(IssueListTemplate { issues })
}

pub async fn search_completions(
    Query(parameters): Query<IssueCompletionQuery>,
) -> Result<Json<IssueCompletionResponse>> {
    let token = parameters.token.trim();
    if token.len() > 128 {
        return Err(AppError::InvalidRequest("Search token is too long"));
    }

    let results = if let Some((key, prefix)) = token.split_once(':') {
        if !key.eq_ignore_ascii_case("state") {
            Vec::new()
        } else {
            ["unresolved", "needs_attention", "resolved", "rejected"]
                .into_iter()
                .filter(|value| value.starts_with(&prefix.to_ascii_lowercase()))
                .map(|value| IssueCompletion {
                    replacement: format!("state:{value}"),
                    label: value.replace('_', " "),
                    description: "Issue state".into(),
                })
                .collect()
        }
    } else {
        [
            ("state", "Workflow state"),
            ("github", "GitHub issue number"),
            ("id", "Reports issue ID"),
        ]
        .into_iter()
        .filter(|(key, _)| key.starts_with(&token.to_ascii_lowercase()))
        .map(|(key, description)| IssueCompletion {
            replacement: format!("{key}:"),
            label: format!("{key}:"),
            description: description.into(),
        })
        .collect()
    };

    Ok(Json(IssueCompletionResponse { results }))
}

fn default_issue_search() -> String {
    "state:unresolved state:needs_attention".into()
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
    state.database.ensure_report_unassigned(report_id).await?;

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
    let issue_state = state
        .database
        .find_issue(issue_id)
        .await?
        .ok_or_else(|| not_found("Issue not found"))?
        .state;

    let github_field_warning = if issue_state == "rejected" {
        false
    } else {
        match ensure_github_reports_link(&state, &session, issue_id).await {
            Ok(()) => false,
            Err(error) => {
                tracing::warn!(event = "github.reports_link_sync_failed", %issue_id, ?error);
                true
            }
        }
    };

    let details = state
        .database
        .issue_details(issue_id)
        .await?
        .ok_or_else(|| not_found("Issue not found"))?;

    let potential_matches = if matches!(
        details.issue.state.as_str(),
        "unresolved" | "needs_attention"
    ) && details.issue.merged_into.is_none()
        && details.issue.resolved_at.is_none()
    {
        state
            .database
            .issue_signature_matches(issue_id)
            .await?
            .into_iter()
            .map(|report| ReportView {
                id: report.id,
                title: report.title,
                client_version: report.client_version,
                created_at: report.created_at,
            })
            .collect()
    } else {
        Vec::new()
    };

    let description_html = render_issue_description(&details.issue.description);
    let issue = IssueView {
        id: details.issue.id,
        title: details.issue.title,
        description: details.issue.description,
        description_html,
        github_number: details.issue.github_number,
        github_url: details.issue.github_url,
        state: details.issue.state,
        github_state: details.issue.github_state,
        merged_into: details.issue.merged_into,
    };

    let reports = details
        .reports
        .into_iter()
        .map(|report| ReportView {
            id: report.id,
            title: report.title,
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
        asset_version: super::assets::asset_version(),
        issue,
        reports,
        potential_matches,
        events,
        github_field_warning,
    }))
}

#[derive(Deserialize)]
pub struct IssueActionForm {
    csrf: String,
}

pub async fn unlink_report(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path((issue_id, report_id)): Path<(IssueId, ReportId)>,
    Form(form): Form<IssueActionForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    state
        .database
        .unlink_report_from_issue(issue_id, report_id, session.github_id)
        .await?;

    tracing::info!(event = "report.update_issue", %report_id, from = %issue_id, to = "none", actor = session.login);
    Ok(Redirect::to(&format!("/issues/{issue_id}")))
}

#[derive(Deserialize)]
pub struct RejectIssueForm {
    csrf: String,
    report_action: String,
}

pub async fn reject(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(issue_id): Path<IssueId>,
    Form(form): Form<RejectIssueForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    let reject_reports = match form.report_action.as_str() {
        "unlink" => false,
        "reject" => true,
        _ => return Err(AppError::InvalidRequest("Invalid report action")),
    };
    let reports_updated = state
        .database
        .reject_issue(issue_id, session.github_id, reject_reports)
        .await?;

    tracing::warn!(
        event = "issue.update_state",
        %issue_id,
        to = "rejected",
        report_action = form.report_action,
        reports_updated,
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
