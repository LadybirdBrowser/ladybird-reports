use std::collections::{HashMap, HashSet};

use axum::{
    Extension, Json,
    extract::{Query, State},
};

use crate::{
    domain::{GithubLinkState, IssueId, IssueState},
    error::{AppError, Result},
    infrastructure::github::GithubIssueState,
};

use super::super::{AdminState, session::Session};
use super::reports::{EntitySearchOption, EntitySearchResponse, ReportSearchQuery};

const TRACKED_ISSUES: &str = "Tracked issues";
const GITHUB_ISSUES: &str = "GitHub issues";
const RESOLVED_ISSUES: &str = "Resolved issues";
const MERGED_PULL_REQUESTS: &str = "Merged pull requests";
const COMMITS: &str = "Commits";

/// How long a GitHub search may take before the selector shows what it has without it.
const GITHUB_SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// The results of a GitHub search the selector shows, or none when it fails or takes too long.
fn github_results<T>(
    outcome: std::result::Result<Result<Vec<T>>, tokio::time::error::Elapsed>,
    search: &'static str,
) -> Vec<T> {
    match outcome {
        Ok(Ok(results)) => results,
        Ok(Err(error)) => {
            tracing::warn!(event = "github.search_failed", search, ?error);
            Vec::new()
        }
        Err(_) => {
            tracing::warn!(event = "github.search_timed_out", search);
            Vec::new()
        }
    }
}

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
    let repository = &configuration.github.repository;
    // A fix is looked for only once the search says something: An empty search lists tracked issues.
    let searches_fixes = !query.is_empty();
    let (tracked_issues, github_issues, pull_requests, commits) = tokio::join!(
        state.database.search_issues(query),
        tokio::time::timeout(
            GITHUB_SEARCH_TIMEOUT,
            state.github.search_issues(&token, repository, query),
        ),
        tokio::time::timeout(GITHUB_SEARCH_TIMEOUT, async {
            if searches_fixes {
                state
                    .github
                    .search_merged_pull_requests(&token, repository, query)
                    .await
            } else {
                Ok(Vec::new())
            }
        }),
        tokio::time::timeout(GITHUB_SEARCH_TIMEOUT, async {
            if searches_fixes {
                state.github.search_commits(&token, repository, query).await
            } else {
                Ok(Vec::new())
            }
        }),
    );
    let tracked_issues = tracked_issues?;
    let github_issues = github_results(github_issues, "issues");
    let pull_requests = github_results(pull_requests, "pull_requests");
    let commits = github_results(commits, "commits");

    let github_numbers = github_issues
        .iter()
        .map(|issue| issue.number)
        .collect::<Vec<_>>();
    let linked_issues = state
        .database
        .issues_linked_to_github_numbers(&configuration.github.repository, &github_numbers)
        .await?
        .into_iter()
        .map(|link| (link.github_number, link))
        .collect::<HashMap<_, _>>();

    let mut included_issue_ids = HashSet::new();
    let mut results = Vec::new();

    for issue in tracked_issues {
        included_issue_ids.insert(issue.id);
        let resolved = issue.state == IssueState::Resolved;
        results.push(EntitySearchOption {
            value: format!("issue:{}", issue.id),
            label: issue.title,
            description: format!("GitHub issue #{}", issue.github_number),
            identifier: format!("#{}", issue.github_number),
            badge: format!("{} reports", issue.report_count),
            badge_tone: if resolved { "resolved" } else { "assigned" },
            footnote: format!("Tracked since {}", issue.created_at.format("%d %b %Y")),
            group: Some(if resolved {
                RESOLVED_ISSUES
            } else {
                TRACKED_ISSUES
            }),
        });
    }

    for github_issue in github_issues {
        if let Some(linked_issue) = linked_issues.get(&github_issue.number) {
            if linked_issue.github_state.is_unreachable() {
                continue;
            }
            let resolved = linked_issue.github_state == GithubLinkState::Closed;
            if included_issue_ids.insert(linked_issue.issue_id) {
                results.push(EntitySearchOption {
                    value: format!("issue:{}", linked_issue.issue_id),
                    label: linked_issue.title.clone(),
                    description: github_issue.title,
                    identifier: format!("#{}", github_issue.number),
                    badge: "Tracked".into(),
                    badge_tone: if resolved { "resolved" } else { "assigned" },
                    footnote: "Already tracked in Reports".into(),
                    group: Some(if resolved {
                        RESOLVED_ISSUES
                    } else {
                        TRACKED_ISSUES
                    }),
                });
            }
            continue;
        }

        // Only an open GitHub issue can start being tracked.
        if github_issue.state != GithubIssueState::Open {
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
            group: Some(GITHUB_ISSUES),
        });
    }

    // A merged pull request or a commit resolves a report that isn't linked to an issue, with the fix as the reason.
    for pull_request in pull_requests {
        results.push(EntitySearchOption {
            value: format!("pull:{}", pull_request.number),
            label: pull_request.title,
            description: pull_request.html_url,
            identifier: format!("#{}", pull_request.number),
            badge: "Merged".into(),
            badge_tone: "resolved",
            footnote: "Resolves the report".into(),
            group: Some(MERGED_PULL_REQUESTS),
        });
    }
    for commit in commits {
        results.push(EntitySearchOption {
            value: format!("commit:{}", commit.sha),
            label: commit.subject().to_owned(),
            description: commit.html_url.clone(),
            identifier: commit.sha.chars().take(11).collect(),
            badge: "Commit".into(),
            badge_tone: "resolved",
            footnote: "Resolves the report, if it's on the default branch".into(),
            group: Some(COMMITS),
        });
    }

    // Open tracked issues come first, and resolved ones last, as a report is rarely added to an issue that is fixed.
    results.sort_by_key(|option| match option.group {
        Some(TRACKED_ISSUES) => 0,
        Some(GITHUB_ISSUES) => 1,
        Some(MERGED_PULL_REQUESTS) => 2,
        Some(COMMITS) => 3,
        _ => 4,
    });

    Ok(Json(EntitySearchResponse { results }))
}

pub(super) async fn ensure_github_reports_link(
    state: &AdminState,
    session: &Session,
    issue_id: IssueId,
) -> Result<()> {
    let configuration = state.database.configuration().await?;
    let Some(field_id) = configuration.github.reports_issue_field_id else {
        return Ok(());
    };
    let issue = state
        .database
        .find_issue(issue_id)
        .await?
        .ok_or(AppError::NotFound("Issue not found"))?;

    if issue.merged_into.is_some() || issue.github_state.is_unreachable() {
        return Ok(());
    }

    let link = format!("{}/issues/{issue_id}", configuration.admin_base_url);
    if issue.github_reports_field_id == Some(field_id)
        && issue.github_reports_link_url.as_deref() == Some(link.as_str())
    {
        return Ok(());
    }

    let organization = configuration
        .github
        .repository
        .split_once('/')
        .expect("validated GitHub repository")
        .0;
    let token = session.github_access_token(state)?;
    state
        .github
        .verify_private_text_issue_field(&token, organization, field_id)
        .await?;
    state
        .github
        .add_issue_field_link(
            &token,
            &issue.github_repository,
            issue.github_number,
            field_id,
            &link,
        )
        .await?;
    state
        .database
        .record_github_reports_link(issue_id, issue.github_issue_id, field_id, &link)
        .await?;

    tracing::info!(event = "github.reports_link_updated", %issue_id, field_id);
    Ok(())
}
