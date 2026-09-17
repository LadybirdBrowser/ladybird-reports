use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::{
    domain::IssueId,
    error::{AppError, Result},
    infrastructure::{database::AdminDatabase, github::GithubIssue},
};

use super::issues::validate_issue_text;

impl AdminDatabase {
    pub async fn replace_github_issue(
        &self,
        issue_id: IssueId,
        repository: &str,
        replacement: &GithubIssue,
        actor: i64,
    ) -> Result<()> {
        if !replacement.belongs_to_repository(repository) {
            return Err(AppError::InvalidRequest(
                "Replacement issue must be in the configured repository",
            ));
        }
        let description = replacement.body.as_deref().unwrap_or_default();
        validate_issue_text(&replacement.title, description)?;

        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        let current = sqlx::query(
            "SELECT github_repository, github_number, github_issue_id,
                    github_url, github_state
             FROM issues
             WHERE id = $1 AND merged_into IS NULL
                AND state <> 'rejected'
             FOR UPDATE",
        )
        .bind(issue_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("Issue not found"))?;

        let old_repository: String = current.get("github_repository");
        let old_number: i64 = current.get("github_number");
        let old_id: Option<i64> = current.get("github_issue_id");
        let old_url: String = current.get("github_url");
        let old_state: String = current.get("github_state");
        if !matches!(old_state.as_str(), "missing" | "moved" | "unavailable") {
            return Err(AppError::Conflict(
                "Current GitHub issue is still available",
            ));
        }
        if old_repository.eq_ignore_ascii_case(repository) && old_number == replacement.number {
            return Err(AppError::Conflict("This GitHub issue is already linked"));
        }

        let already_linked: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM issues
                WHERE id <> $1 AND state <> 'rejected'
                    AND (
                        (lower(github_repository) = lower($2) AND github_number = $3)
                        OR github_issue_id = $4
                    )
                UNION ALL
                SELECT 1 FROM issue_github_aliases AS aliases
                JOIN issues ON issues.id = aliases.issue_id
                WHERE issues.state <> 'rejected'
                    AND ((lower(aliases.github_repository) = lower($2)
                        AND aliases.github_number = $3)
                        OR aliases.github_issue_id = $4)
             )",
        )
        .bind(issue_id)
        .bind(repository)
        .bind(replacement.number)
        .bind(replacement.id)
        .fetch_one(&mut *transaction)
        .await?;
        if already_linked {
            return Err(AppError::Conflict(
                "Replacement GitHub issue is already tracked",
            ));
        }

        sqlx::query(
            "INSERT INTO issue_github_aliases (
                issue_id, github_repository, github_number,
                github_issue_id, github_url
             ) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(issue_id)
        .bind(&old_repository)
        .bind(old_number)
        .bind(old_id)
        .bind(&old_url)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "UPDATE issues
             SET github_repository = $2,
                 github_number = $3,
                 github_issue_id = $4,
                 github_url = $5,
                 title = $6,
                 description = $7,
                 github_state = $8,
                 state = CASE WHEN $8 = 'closed' THEN 'resolved' ELSE 'unresolved' END,
                 github_checked_at = now(),
                 github_updated_at = $9,
                 github_reports_field_id = NULL,
                 github_reports_link_url = NULL,
                 resolved_at = CASE
                    WHEN $8 = 'closed' THEN COALESCE(resolved_at, now())
                    ELSE NULL
                 END,
                 updated_at = now()
             WHERE id = $1",
        )
        .bind(issue_id)
        .bind(repository)
        .bind(replacement.number)
        .bind(replacement.id)
        .bind(&replacement.html_url)
        .bind(&replacement.title)
        .bind(description)
        .bind(replacement.state.as_str())
        .bind(replacement.updated_at)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id, details)
             VALUES ($1, 'issue.update_github_link', $2, $3)",
        )
        .bind(actor)
        .bind(issue_id.0)
        .bind(serde_json::json!({
            "from": { "repository": old_repository, "number": old_number, "url": old_url },
            "to": { "repository": repository, "number": replacement.number,
                    "url": replacement.html_url },
        }))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn sync_github_issue(
        &self,
        repository: &str,
        issue: &GithubIssue,
        actor: Option<i64>,
        source: &str,
    ) -> Result<Option<IssueId>> {
        let description = issue.body.as_deref().unwrap_or_default();
        validate_issue_text(&issue.title, description)?;
        let current_repository = issue.repository().ok_or(AppError::InvalidRequest(
            "GitHub returned an invalid issue URL",
        ))?;
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT id, github_issue_id, github_state, title, description,
                    github_repository, github_url, github_updated_at
             FROM issues
             WHERE state <> 'rejected'
                AND (github_issue_id = $1
                    OR (lower(github_repository) = lower($2) AND github_number = $3))
             ORDER BY COALESCE(github_issue_id = $1, false) DESC
             LIMIT 1
             FOR UPDATE",
        )
        .bind(issue.id)
        .bind(repository)
        .bind(issue.number)
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let issue_id: IssueId = row.get("id");
        let stored_repository: String = row.get("github_repository");
        let state = if current_repository.eq_ignore_ascii_case(&stored_repository) {
            issue.state.as_str()
        } else {
            "moved"
        };
        let previous_id: Option<i64> = row.get("github_issue_id");
        if previous_id.is_some_and(|id| id != issue.id) {
            return Err(AppError::Conflict("GitHub issue identity changed"));
        }

        let previous_update: Option<DateTime<Utc>> = row.get("github_updated_at");
        if previous_update.is_some_and(|updated| updated > issue.updated_at) {
            transaction.commit().await?;
            return Ok(Some(issue_id));
        }

        let previous_state: String = row.get("github_state");
        if source == "webhook" && matches!(previous_state.as_str(), "missing" | "moved") {
            transaction.commit().await?;
            return Ok(Some(issue_id));
        }
        let previous_title: String = row.get("title");
        let previous_description: String = row.get("description");
        let previous_url: String = row.get("github_url");
        sqlx::query(
            "UPDATE issues
             SET github_issue_id = $2,
                 title = $3,
                 description = $4,
                 github_url = $5,
                 github_state = $6,
                 state = CASE
                    WHEN $6 IN ('missing', 'moved', 'unavailable') THEN 'needs_attention'
                    WHEN $6 = 'closed' THEN 'resolved'
                    ELSE 'unresolved'
                 END,
                 github_checked_at = now(),
                 github_updated_at = $7,
                 resolved_at = CASE
                    WHEN $6 = 'open' THEN NULL
                    WHEN $6 = 'closed' THEN COALESCE(resolved_at, now())
                    ELSE resolved_at
                 END,
                 updated_at = CASE
                    WHEN github_state IS DISTINCT FROM $6
                        OR title IS DISTINCT FROM $3
                        OR description IS DISTINCT FROM $4
                        OR github_url IS DISTINCT FROM $5
                    THEN now()
                    ELSE updated_at
                 END
             WHERE id = $1",
        )
        .bind(issue_id)
        .bind(issue.id)
        .bind(&issue.title)
        .bind(description)
        .bind(&issue.html_url)
        .bind(state)
        .bind(issue.updated_at)
        .execute(&mut *transaction)
        .await?;

        if previous_state != state
            || previous_title != issue.title
            || previous_description != description
            || previous_url != issue.html_url
        {
            sqlx::query(
                "INSERT INTO audit_events (actor, action, entity_id, details)
                 VALUES ($1, 'issue.sync_github', $2, $3)",
            )
            .bind(actor)
            .bind(issue_id.0)
            .bind(serde_json::json!({
                "source": source,
                "state": { "from": previous_state, "to": state },
                "title_changed": previous_title != issue.title,
                "description_changed": previous_description != description,
                "url_changed": previous_url != issue.html_url,
            }))
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(Some(issue_id))
    }

    pub async fn mark_github_issue_unavailable(
        &self,
        repository: &str,
        number: i64,
        github_id: Option<i64>,
        state: &str,
        source: &str,
    ) -> Result<Option<IssueId>> {
        if !matches!(state, "missing" | "moved" | "unavailable") {
            return Err(AppError::InvalidRequest("Invalid GitHub link state"));
        }

        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT id, github_state, github_issue_id
             FROM issues
             WHERE state <> 'rejected'
                AND (github_issue_id = $1
                    OR (lower(github_repository) = lower($2) AND github_number = $3))
             ORDER BY COALESCE(github_issue_id = $1, false) DESC
             LIMIT 1
             FOR UPDATE",
        )
        .bind(github_id)
        .bind(repository)
        .bind(number)
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let issue_id: IssueId = row.get("id");
        let previous_id: Option<i64> = row.get("github_issue_id");
        if github_id.is_some_and(|id| previous_id.is_some_and(|previous| previous != id)) {
            return Err(AppError::Conflict("GitHub issue identity changed"));
        }
        let previous_state: String = row.get("github_state");

        sqlx::query(
            "UPDATE issues
             SET github_issue_id = COALESCE(github_issue_id, $2),
                 github_state = $3,
                 state = 'needs_attention',
                 github_checked_at = now(),
                 updated_at = CASE WHEN github_state <> $3 THEN now() ELSE updated_at END
             WHERE id = $1",
        )
        .bind(issue_id)
        .bind(github_id)
        .bind(state)
        .execute(&mut *transaction)
        .await?;

        if previous_state != state {
            sqlx::query(
                "INSERT INTO audit_events (action, entity_id, details)
                 VALUES ('issue.sync_github', $1, $2)",
            )
            .bind(issue_id.0)
            .bind(serde_json::json!({
                "source": source,
                "state": { "from": previous_state, "to": state },
            }))
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(Some(issue_id))
    }

    pub async fn record_github_reports_link(
        &self,
        issue_id: IssueId,
        github_issue_id: Option<i64>,
        field_id: i64,
        link: &str,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE issues
             SET github_reports_field_id = $3,
                 github_reports_link_url = $4
             WHERE id = $1 AND github_issue_id IS NOT DISTINCT FROM $2
                 AND merged_into IS NULL
                 AND state <> 'rejected'",
        )
        .bind(issue_id)
        .bind(github_issue_id)
        .bind(field_id)
        .bind(link)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() != 1 {
            return Err(AppError::Conflict("GitHub issue link changed during sync"));
        }

        Ok(())
    }
}
