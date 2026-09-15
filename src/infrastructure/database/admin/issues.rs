use sqlx::Row;

use crate::{
    domain::{GithubPublishAttemptId, IssueId, ReportId},
    error::{AppError, Result},
    infrastructure::database::AdminDatabase,
};

use super::{IssueDetails, IssueRecord, IssueSummary, ReportSummary};

impl AdminDatabase {
    pub async fn begin_github_publish(
        &self,
        issue_id: IssueId,
        actor: i64,
    ) -> Result<GithubPublishAttemptId> {
        let attempt_id = GithubPublishAttemptId::new();
        let result = sqlx::query(
            "INSERT INTO github_publish_attempts (id, issue_id, actor, state)
             SELECT $1, id, $3, 'pending'
             FROM issues
             WHERE id = $2
                AND merged_into IS NULL
                AND github_number IS NULL
             ON CONFLICT DO NOTHING",
        )
        .bind(attempt_id)
        .bind(issue_id)
        .bind(actor)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() != 1 {
            return Err(AppError::InvalidRequest(
                "Issue is already linked or has an unfinished GitHub publish",
            ));
        }

        Ok(attempt_id)
    }

    pub async fn mark_github_publish_uncertain(
        &self,
        attempt_id: GithubPublishAttemptId,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE github_publish_attempts
             SET state = 'uncertain'
             WHERE id = $1 AND state = 'pending'",
        )
        .bind(attempt_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn mark_github_publish_failed(
        &self,
        attempt_id: GithubPublishAttemptId,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE github_publish_attempts
             SET state = 'failed'
             WHERE id = $1 AND state = 'pending'",
        )
        .bind(attempt_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn clear_uncertain_github_publish(
        &self,
        issue_id: IssueId,
        actor: i64,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE github_publish_attempts
             SET state = 'failed'
             WHERE issue_id = $1 AND state = 'uncertain'",
        )
        .bind(issue_id)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() != 1 {
            return Err(AppError::InvalidRequest(
                "Issue does not have an uncertain GitHub publish",
            ));
        }

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id)
             VALUES ($1, 'issue.github_publish_cleared', $2)",
        )
        .bind(actor)
        .bind(issue_id.0)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn finish_github_publish(
        &self,
        attempt_id: GithubPublishAttemptId,
        issue_id: IssueId,
        github_number: i64,
        github_url: &str,
        actor: i64,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE issues
             SET github_number = $2, github_url = $3, updated_at = now()
             WHERE id = $1 AND github_number IS NULL AND merged_into IS NULL",
        )
        .bind(issue_id)
        .bind(github_number)
        .bind(github_url)
        .execute(&mut *transaction)
        .await?;

        if updated.rows_affected() != 1 {
            return Err(AppError::InvalidRequest("Issue can no longer be linked"));
        }

        sqlx::query(
            "UPDATE github_publish_attempts
             SET state = 'succeeded'
             WHERE id = $1 AND issue_id = $2 AND state = 'pending'",
        )
        .bind(attempt_id)
        .bind(issue_id)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id, details)
             VALUES ($1, 'issue.github_published', $2, $3)",
        )
        .bind(actor)
        .bind(issue_id.0)
        .bind(serde_json::json!({
            "number": github_number,
            "url": github_url,
        }))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_issues(&self, include_resolved: bool) -> Result<Vec<IssueSummary>> {
        let rows = sqlx::query(
            "SELECT
                issues.id,
                issues.title,
                issues.resolved_at,
                issues.github_number,
                issues.created_at,
                count(reports.id) AS report_count
             FROM issues
             LEFT JOIN reports
                ON reports.issue_id = issues.id
                AND reports.deleted_at IS NULL
                AND reports.storage_state = 'ready'
             WHERE issues.merged_into IS NULL
                AND ($1 OR issues.resolved_at IS NULL)
             GROUP BY issues.id
             ORDER BY issues.updated_at DESC
             LIMIT 200",
        )
        .bind(include_resolved)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| IssueSummary {
                id: row.get("id"),
                title: row.get("title"),
                resolved_at: row.get("resolved_at"),
                github_number: row.get("github_number"),
                report_count: row.get("report_count"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    pub async fn create_issue(
        &self,
        title: &str,
        description: &str,
        report_ids: &[ReportId],
        actor: i64,
    ) -> Result<IssueId> {
        validate_issue_text(title, description)?;

        let issue_id = IssueId::new();
        let mut transaction = self.pool.begin().await?;

        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        sqlx::query("INSERT INTO issues (id, title, description) VALUES ($1, $2, $3)")
            .bind(issue_id)
            .bind(title.trim())
            .bind(description)
            .execute(&mut *transaction)
            .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id)
             VALUES ($1, 'issue.created', $2)",
        )
        .bind(actor)
        .bind(issue_id.0)
        .execute(&mut *transaction)
        .await?;

        for report_id in report_ids {
            let updated = sqlx::query(
                "UPDATE reports
                 SET issue_id = $2, assigned_at = now(), updated_at = now()
                 WHERE id = $1 AND deleted_at IS NULL AND storage_state = 'ready'",
            )
            .bind(report_id)
            .bind(issue_id)
            .execute(&mut *transaction)
            .await?;

            if updated.rows_affected() != 1 {
                return Err(AppError::NotFound("Report not found"));
            }

            sqlx::query(
                "INSERT INTO audit_events (actor, action, entity_id, details)
                 VALUES ($1, 'report.assignment', $2, $3)",
            )
            .bind(actor)
            .bind(report_id.0)
            .bind(serde_json::json!({ "to": issue_id }))
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(issue_id)
    }

    pub async fn issue_details(&self, issue_id: IssueId) -> Result<Option<IssueDetails>> {
        let issue = self.find_issue(issue_id).await?;

        let Some(issue) = issue else {
            return Ok(None);
        };

        let report_rows = sqlx::query(
            "SELECT id, client_version, build, issue_id, created_at
             FROM reports
             WHERE issue_id = $1
                AND deleted_at IS NULL
                AND storage_state = 'ready'
             ORDER BY created_at DESC",
        )
        .bind(issue_id)
        .fetch_all(&self.pool)
        .await?;

        let reports = report_rows
            .into_iter()
            .map(|row| ReportSummary {
                id: row.get("id"),
                client_version: row.get("client_version"),
                build: row.get("build"),
                issue_id: row.get("issue_id"),
                created_at: row.get("created_at"),
            })
            .collect();

        let events = self.audit_events(issue_id.0).await?;

        Ok(Some(IssueDetails {
            issue,
            reports,
            events,
        }))
    }

    pub async fn find_issue(&self, issue_id: IssueId) -> Result<Option<IssueRecord>> {
        let row = sqlx::query(
            "SELECT
                issues.id,
                issues.title,
                issues.description,
                issues.resolved_at,
                issues.merged_into,
                issues.github_number,
                issues.github_url,
                issues.created_at,
                issues.updated_at,
                (
                    SELECT state
                    FROM github_publish_attempts
                    WHERE issue_id = issues.id
                        AND state IN ('pending', 'uncertain')
                    ORDER BY created_at DESC
                    LIMIT 1
                ) AS github_publish_state
             FROM issues
             WHERE issues.id = $1",
        )
        .bind(issue_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| IssueRecord {
            id: row.get("id"),
            title: row.get("title"),
            description: row.get("description"),
            resolved_at: row.get("resolved_at"),
            merged_into: row.get("merged_into"),
            github_number: row.get("github_number"),
            github_url: row.get("github_url"),
            github_publish_state: row.get("github_publish_state"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        }))
    }

    pub async fn update_issue(
        &self,
        issue_id: IssueId,
        title: &str,
        description: &str,
        resolved: bool,
        actor: i64,
    ) -> Result<()> {
        validate_issue_text(title, description)?;

        let result = sqlx::query(
            "UPDATE issues
             SET
                title = $2,
                description = $3,
                resolved_at = CASE
                    WHEN $4 AND resolved_at IS NULL THEN now()
                    WHEN NOT $4 THEN NULL
                    ELSE resolved_at
                END,
                updated_at = now()
             WHERE id = $1 AND merged_into IS NULL",
        )
        .bind(issue_id)
        .bind(title.trim())
        .bind(description)
        .bind(resolved)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() != 1 {
            return Err(AppError::NotFound("Issue not found"));
        }

        self.insert_audit_event(
            actor,
            "issue.updated",
            issue_id.0,
            serde_json::json!({ "resolved": resolved }),
        )
        .await
    }

    pub async fn merge_issue(
        &self,
        source: IssueId,
        destination: IssueId,
        actor: i64,
    ) -> Result<()> {
        if source == destination {
            return Err(AppError::InvalidRequest(
                "An issue cannot be merged into itself",
            ));
        }

        let mut transaction = self.pool.begin().await?;

        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        let destination_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM issues WHERE id = $1 AND merged_into IS NULL
            )",
        )
        .bind(destination)
        .fetch_one(&mut *transaction)
        .await?;

        if !destination_exists {
            return Err(AppError::InvalidRequest("Destination issue is not active"));
        }

        let source_update = sqlx::query(
            "UPDATE issues
             SET merged_into = $2, updated_at = now()
             WHERE id = $1 AND merged_into IS NULL",
        )
        .bind(source)
        .bind(destination)
        .execute(&mut *transaction)
        .await?;

        if source_update.rows_affected() != 1 {
            return Err(AppError::NotFound("Source issue not found"));
        }

        sqlx::query(
            "UPDATE reports
             SET issue_id = $2, updated_at = now()
             WHERE issue_id = $1",
        )
        .bind(source)
        .bind(destination)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id, details)
             VALUES ($1, 'issue.merged', $2, $3)",
        )
        .bind(actor)
        .bind(source.0)
        .bind(serde_json::json!({ "into": destination }))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn link_github_issue(
        &self,
        issue_id: IssueId,
        github_number: i64,
        github_url: &str,
        actor: i64,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE issues
             SET github_number = $2, github_url = $3, updated_at = now()
             WHERE id = $1 AND merged_into IS NULL",
        )
        .bind(issue_id)
        .bind(github_number)
        .bind(github_url)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() != 1 {
            return Err(AppError::NotFound("Issue not found"));
        }

        sqlx::query(
            "UPDATE github_publish_attempts
             SET state = 'succeeded'
             WHERE issue_id = $1 AND state IN ('pending', 'uncertain')",
        )
        .bind(issue_id)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id, details)
             VALUES ($1, 'issue.github_linked', $2, $3)",
        )
        .bind(actor)
        .bind(issue_id.0)
        .bind(serde_json::json!({
            "number": github_number,
            "url": github_url,
        }))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    async fn insert_audit_event(
        &self,
        actor: i64,
        action: &str,
        entity_id: uuid::Uuid,
        details: serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id, details)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(actor)
        .bind(action)
        .bind(entity_id)
        .bind(details)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

fn validate_issue_text(title: &str, description: &str) -> Result<()> {
    if title.trim().is_empty() || title.len() > 256 {
        return Err(AppError::InvalidRequest(
            "Issue title must be 1 to 256 bytes",
        ));
    }

    if description.len() > 256 * 1024 {
        return Err(AppError::InvalidRequest("Issue description is too large"));
    }

    Ok(())
}
