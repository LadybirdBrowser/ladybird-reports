use std::collections::{HashMap, HashSet};

use axum::{
    Extension, Json,
    extract::{Query, State},
};

use crate::{
    domain::{GithubLinkState, IssueId, IssueState},
    error::{AppError, Result},
    infrastructure::github::{GithubCommit, GithubIssueState},
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
    let (tracked_issues, github_issues) = tokio::join!(
        state.database.search_issues(query),
        tokio::time::timeout(
            GITHUB_SEARCH_TIMEOUT,
            state.github.search_issues(&token, repository, query),
        ),
    );
    let tracked_issues = tracked_issues?;
    let github_issues = github_results(github_issues, "issues");

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

    // Open tracked issues come first, and resolved ones last, as a report is rarely added to an issue that is fixed.
    results.sort_by_key(|option| match option.group {
        Some(TRACKED_ISSUES) => 0,
        Some(GITHUB_ISSUES) => 1,
        _ => 2,
    });

    Ok(Json(EntitySearchResponse { results }))
}

/// The commits the default branch contains, which are the ones that resolve a report. The checks run together, and a
/// commit whose check fails or takes too long isn’t offered.
async fn commits_on_default_branch(
    state: &AdminState,
    token: &str,
    repository: &str,
    commits: Vec<GithubCommit>,
) -> Vec<GithubCommit> {
    if commits.is_empty() {
        return commits;
    }
    let branch = match tokio::time::timeout(
        GITHUB_SEARCH_TIMEOUT,
        state.github.default_branch(token, repository),
    )
    .await
    {
        Ok(Ok(branch)) => branch,
        Ok(Err(error)) => {
            tracing::warn!(event = "github.default_branch_failed", ?error);
            return Vec::new();
        }
        Err(_) => {
            tracing::warn!(event = "github.default_branch_timed_out");
            return Vec::new();
        }
    };

    let mut checks = tokio::task::JoinSet::new();
    for (index, commit) in commits.iter().enumerate() {
        let github = state.github.clone();
        let (token, repository, branch, sha) = (
            token.to_owned(),
            repository.to_owned(),
            branch.clone(),
            commit.sha.clone(),
        );
        checks.spawn(async move {
            let contains = tokio::time::timeout(
                GITHUB_SEARCH_TIMEOUT,
                github.branch_contains(&token, &repository, &branch, &sha),
            )
            .await;
            (index, matches!(contains, Ok(Ok(true))))
        });
    }
    let mut on_branch = vec![false; commits.len()];
    while let Some(Ok((index, contains))) = checks.join_next().await {
        on_branch[index] = contains;
    }
    commits
        .into_iter()
        .zip(on_branch)
        .filter_map(|(commit, contains)| contains.then_some(commit))
        .collect()
}

/// The fixes the fix selector offers for a search: merged pull requests, and commits on the repository's default branch,
/// by words from their titles, a pull request number, or a commit hash.
pub async fn fix_options(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Query(parameters): Query<ReportSearchQuery>,
) -> Result<Json<EntitySearchResponse>> {
    let query = parameters.query.trim();
    if query.len() > 128 {
        return Err(AppError::InvalidRequest("Fix search is too long"));
    }
    if query.is_empty() {
        return Ok(Json(EntitySearchResponse {
            results: Vec::new(),
        }));
    }

    let configuration = state.database.configuration().await?;
    let token = session.github_access_token(&state)?;
    let repository = &configuration.github.repository;
    let (pull_requests, commits) = tokio::join!(
        tokio::time::timeout(
            GITHUB_SEARCH_TIMEOUT,
            state
                .github
                .search_merged_pull_requests(&token, repository, query),
        ),
        tokio::time::timeout(
            GITHUB_SEARCH_TIMEOUT,
            state.github.search_commits(&token, repository, query),
        ),
    );
    // A search can name another repository with its own qualifier, while a choice carries only a number or a hash
    // that's looked up in the configured repository: Only that repository's results are offered.
    let pull_requests = github_results(pull_requests, "pull_requests")
        .into_iter()
        .filter(|pull_request| {
            pull_request
                .html_url
                .contains(&format!("/{repository}/pull/"))
        })
        .collect::<Vec<_>>();
    let commits = github_results(commits, "commits")
        .into_iter()
        .filter(|commit| commit.html_url.contains(&format!("/{repository}/commit/")))
        .collect::<Vec<_>>();
    let commits = commits_on_default_branch(&state, &token, repository, commits).await;

    let mut results = Vec::new();
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
            footnote: "Resolves the report".into(),
            group: Some(COMMITS),
        });
    }

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
