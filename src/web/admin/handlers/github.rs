use std::collections::{HashMap, HashSet};

use axum::{
    Extension, Json,
    extract::{Query, State},
};

use crate::error::{AppError, Result};

use super::super::{AdminState, session::Session};
use super::reports::{EntitySearchOption, EntitySearchResponse, ReportSearchQuery};

pub async fn issue_options(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Query(parameters): Query<ReportSearchQuery>,
) -> Result<Json<EntitySearchResponse>> {
    let query = parameters.query.trim();
    if query.len() > 128 {
        return Err(AppError::InvalidRequest("Issue search is too long"));
    }

    let configuration = state.database.configuration().await?;
    let token = session.github_access_token(&state)?;
    let (tracked_issues, github_issues) = tokio::try_join!(
        state.database.search_issues(query),
        state
            .github
            .search_issues(&token, &configuration.github_repository, query),
    )?;

    let github_numbers = github_issues
        .iter()
        .map(|issue| issue.number)
        .collect::<Vec<_>>();
    let linked_issues = state
        .database
        .issues_linked_to_github_numbers(&github_numbers)
        .await?
        .into_iter()
        .map(|link| (link.github_number, link))
        .collect::<HashMap<_, _>>();

    let mut included_issue_ids = HashSet::new();
    let mut results = Vec::new();

    for issue in tracked_issues {
        included_issue_ids.insert(issue.id);
        results.push(EntitySearchOption {
            value: format!("issue:{}", issue.id),
            label: issue.title,
            description: format!("GitHub issue #{}", issue.github_number),
            identifier: format!("#{}", issue.github_number),
            badge: format!("{} reports", issue.report_count),
            badge_tone: "assigned",
            footnote: format!("Tracked since {}", issue.created_at.format("%d %b %Y")),
            group: Some("Tracked issues"),
        });
    }

    for github_issue in github_issues {
        if let Some(linked_issue) = linked_issues.get(&github_issue.number) {
            if included_issue_ids.insert(linked_issue.issue_id) {
                results.push(EntitySearchOption {
                    value: format!("issue:{}", linked_issue.issue_id),
                    label: linked_issue.title.clone(),
                    description: github_issue.title,
                    identifier: format!("#{}", github_issue.number),
                    badge: "Tracked".into(),
                    badge_tone: "assigned",
                    footnote: "Already tracked in Reports".into(),
                    group: Some("Tracked issues"),
                });
            }
            continue;
        }

        results.push(EntitySearchOption {
            value: format!("github:{}", github_issue.number),
            label: github_issue.title,
            description: github_issue.html_url,
            identifier: format!("#{}", github_issue.number),
            badge: "GitHub".into(),
            badge_tone: "neutral",
            footnote: "Not yet tracked in Reports".into(),
            group: Some("GitHub issues"),
        });
    }

    results.sort_by_key(|option| option.group != Some("Tracked issues"));

    Ok(Json(EntitySearchResponse { results }))
}

pub(super) fn github_body(description: &str, report_count: usize) -> String {
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
