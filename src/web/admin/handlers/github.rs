use askama::Template;
use axum::{
    Extension, Form, Json,
    extract::{Path, Query, State},
    response::Redirect,
};
use serde::Deserialize;

use crate::{
    domain::IssueId,
    error::{AppError, Result},
};

use super::super::{
    AdminState, TemplateResponse, authentication::Navigation, session::Session,
    templates::not_found,
};
use super::reports::{EntitySearchOption, EntitySearchResponse, ReportSearchQuery};

pub async fn search_options(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Query(parameters): Query<ReportSearchQuery>,
) -> Result<Json<EntitySearchResponse>> {
    let query = parameters.query.trim();
    if query.len() > 128 {
        return Err(AppError::InvalidRequest("GitHub issue search is too long"));
    }

    let configuration = state.database.configuration().await?;
    let token = session.github_access_token(&state)?;
    let results = state
        .github
        .search_issues(&token, &configuration.github_repository, query)
        .await?
        .into_iter()
        .map(|issue| EntitySearchOption {
            value: issue.number.to_string(),
            label: issue.title,
            description: issue.html_url,
            identifier: format!("#{}", issue.number),
            badge: "GitHub".into(),
            badge_tone: "neutral",
            footnote: "Existing issue".into(),
        })
        .collect();

    Ok(Json(EntitySearchResponse { results }))
}

#[derive(Template)]
#[template(path = "issues/github-preview.html")]
pub struct GithubPreviewTemplate {
    navigation: Option<Navigation>,
    issue_id: IssueId,
    title: String,
    body: String,
}

#[derive(Deserialize)]
pub struct LinkForm {
    csrf: String,
    number: i64,
}

pub async fn link(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(issue_id): Path<IssueId>,
    Form(form): Form<LinkForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;

    if form.number < 1 {
        return Err(AppError::InvalidRequest("Invalid GitHub issue number"));
    }

    let configuration = state.database.configuration().await?;
    let access_token = session.github_access_token(&state)?;
    let github_issue = state
        .github
        .issue(&access_token, &configuration.github_repository, form.number)
        .await?;

    state
        .database
        .link_github_issue(
            issue_id,
            github_issue.number,
            &github_issue.html_url,
            session.github_id,
        )
        .await?;

    tracing::info!(
        event = "github.issue_linked",
        %issue_id,
        github_number = github_issue.number,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/issues/{issue_id}")))
}

pub async fn preview(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(issue_id): Path<IssueId>,
) -> Result<TemplateResponse<GithubPreviewTemplate>> {
    let details = state
        .database
        .issue_details(issue_id)
        .await?
        .ok_or_else(|| not_found("Issue not found"))?;

    if details.issue.github_number.is_some() {
        return Err(AppError::InvalidRequest(
            "Issue is already linked to GitHub",
        ));
    }

    Ok(TemplateResponse(GithubPreviewTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        issue_id,
        title: details.issue.title,
        body: github_body(&details.issue.description, details.reports.len()),
    }))
}

#[derive(Deserialize)]
pub struct PublishForm {
    csrf: String,
    title: String,
    body: String,
}

pub async fn publish(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(issue_id): Path<IssueId>,
    Form(form): Form<PublishForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;

    if form.title.trim().is_empty() || form.title.len() > 256 {
        return Err(AppError::InvalidRequest("Invalid GitHub issue title"));
    }

    if form.body.len() > 256 * 1024 {
        return Err(AppError::InvalidRequest("GitHub issue body is too large"));
    }

    let configuration = state.database.configuration().await?;
    let access_token = session.github_access_token(&state)?;
    let attempt_id = state
        .database
        .begin_github_publish(issue_id, session.github_id)
        .await?;

    let github_issue = match state
        .github
        .create_issue(
            &access_token,
            &configuration.github_repository,
            form.title.trim(),
            &form.body,
        )
        .await
    {
        Ok(issue) => issue,
        Err(error) => {
            let outcome_is_uncertain =
                matches!(error, AppError::Internal(_) | AppError::Unavailable);

            if outcome_is_uncertain {
                // A network failure can happen after GitHub accepted the request.
                // Keep the attempt visible so an operator can reconcile it instead
                // of allowing an automatic retry that might create a duplicate.
                state
                    .database
                    .mark_github_publish_uncertain(attempt_id)
                    .await?;
                tracing::warn!(
                    event = "github.issue_create_uncertain",
                    %issue_id,
                    actor = session.login,
                );
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

    tracing::info!(
        event = "github.issue_created",
        %issue_id,
        github_number = github_issue.number,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/issues/{issue_id}")))
}

#[derive(Deserialize)]
pub struct ClearUncertainForm {
    csrf: String,
}

pub async fn clear_uncertain(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(issue_id): Path<IssueId>,
    Form(form): Form<ClearUncertainForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    state
        .database
        .clear_uncertain_github_publish(issue_id, session.github_id)
        .await?;

    tracing::warn!(
        event = "github.issue_create_uncertain_cleared",
        %issue_id,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/issues/{issue_id}/github/preview")))
}

fn github_body(description: &str, report_count: usize) -> String {
    let summary = if description.trim().is_empty() {
        "Reports collected by the Ladybird reporting service.".to_owned()
    } else {
        description.to_owned()
    };

    format!(
        "{summary}\n\n---\nLinked reports: {report_count}\n\n\
         Report data is available in the reporting service."
    )
}

#[cfg(test)]
mod tests {
    use super::github_body;

    #[test]
    fn github_body_preserves_the_description_and_counts_reports() {
        let body = github_body("A reproducible rendering problem.", 3);

        assert!(body.starts_with("A reproducible rendering problem."));
        assert!(body.contains("Linked reports: 3"));
    }
}
