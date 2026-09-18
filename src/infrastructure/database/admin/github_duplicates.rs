use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::{
    domain::IssueId,
    error::{AppError, Result},
    infrastructure::{database::AdminDatabase, github::GithubIssue},
};

use super::issues::validate_issue_text;

pub struct GithubDuplicateJob {
    pub source_issue_id: IssueId,
    pub github_issue_id: i64,
    pub repository: String,
    pub number: i64,
    pub installation_id: i64,
    pub lease_id: uuid::Uuid,
    pub attempt_count: i32,
}

impl AdminDatabase {
    pub async fn claim_github_duplicate(&self) -> Result<Option<GithubDuplicateJob>> {
        let lease_id = uuid::Uuid::now_v7();
        let row = sqlx::query(
            "UPDATE github_duplicate_jobs
             SET lease_id = $1,
                 lease_expires_at = now() + interval '2 minutes',
                 attempt_count = attempt_count + 1
             WHERE source_issue_id = (
                SELECT source_issue_id FROM github_duplicate_jobs
                WHERE next_attempt_at <= now()
                    AND (lease_expires_at IS NULL OR lease_expires_at < now())
                ORDER BY next_attempt_at
                FOR UPDATE SKIP LOCKED
                LIMIT 1
             )
             RETURNING source_issue_id, github_issue_id, github_repository,
                       github_number, installation_id, attempt_count",
        )
        .bind(lease_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| GithubDuplicateJob {
            source_issue_id: row.get("source_issue_id"),
            github_issue_id: row.get("github_issue_id"),
            repository: row.get("github_repository"),
            number: row.get("github_number"),
            installation_id: row.get("installation_id"),
            lease_id,
            attempt_count: row.get("attempt_count"),
        }))
    }

    pub async fn retry_github_duplicate(&self, job: &GithubDuplicateJob) -> Result<()> {
        let delay = 30_i64.saturating_mul(2_i64.pow(job.attempt_count.clamp(1, 8) as u32));
        sqlx::query(
            "UPDATE github_duplicate_jobs
             SET lease_id = NULL, lease_expires_at = NULL,
                 next_attempt_at = now() + make_interval(secs => $3)
             WHERE source_issue_id = $1 AND lease_id = $2",
        )
        .bind(job.source_issue_id)
        .bind(job.lease_id)
        .bind(delay.min(3600) as f64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_github_duplicate(
        &self,
        job: &GithubDuplicateJob,
        canonical: Option<&GithubIssue>,
    ) -> Result<Option<IssueId>> {
        let canonical_repository = if let Some(canonical) = canonical {
            validate_issue_text(
                &canonical.title,
                canonical.body.as_deref().unwrap_or_default(),
            )?;
            Some(canonical.repository().ok_or(AppError::InvalidRequest(
                "GitHub returned an invalid duplicate target URL",
            ))?)
        } else {
            None
        };

        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        let lease_matches: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM github_duplicate_jobs
                WHERE source_issue_id = $1 AND lease_id = $2
             )",
        )
        .bind(job.source_issue_id)
        .bind(job.lease_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !lease_matches {
            return Ok(None);
        }

        let source = sqlx::query(
            "SELECT github_issue_id, github_state, merged_into
             FROM issues WHERE id = $1 FOR UPDATE",
        )
        .bind(job.source_issue_id)
        .fetch_optional(&mut *transaction)
        .await?;

        let source_is_current = source.is_some_and(|source| {
            source.get::<Option<i64>, _>("github_issue_id") == Some(job.github_issue_id)
                && source.get::<String, _>("github_state") == "closed"
                && source.get::<Option<IssueId>, _>("merged_into").is_none()
        });
        let destination = if source_is_current {
            if let Some(canonical) = canonical {
                if canonical.id == job.github_issue_id {
                    return Err(AppError::Conflict("GitHub issue duplicates itself"));
                }
                Some(
                    ensure_destination(
                        &mut transaction,
                        job.source_issue_id,
                        canonical_repository
                            .as_deref()
                            .expect("repository was validated"),
                        canonical,
                    )
                    .await?,
                )
            } else {
                None
            }
        } else {
            None
        };

        if let Some(destination) = destination {
            sqlx::query(
                "WITH moved AS (
                    UPDATE reports
                    SET issue_id = $2, state = 'confirmed', updated_at = now()
                    WHERE issue_id = $1 RETURNING id
                 )
                 INSERT INTO audit_events (action, entity_id, details)
                 SELECT 'report.update_issue', id,
                    jsonb_build_object('from', $1::uuid, 'to', $2::uuid)
                 FROM moved",
            )
            .bind(job.source_issue_id)
            .bind(destination)
            .execute(&mut *transaction)
            .await?;

            sqlx::query(
                "UPDATE issues SET merged_into = $2, state = 'resolved',
                    resolved_at = COALESCE(resolved_at, now()), updated_at = now()
                 WHERE id = $1",
            )
            .bind(job.source_issue_id)
            .bind(destination)
            .execute(&mut *transaction)
            .await?;

            sqlx::query(
                "INSERT INTO audit_events (action, entity_id, details)
                 VALUES ('issue.merge', $1, $2)",
            )
            .bind(job.source_issue_id.0)
            .bind(serde_json::json!({
                "source": "github_duplicate",
                "into": destination,
                "github_number": job.number,
            }))
            .execute(&mut *transaction)
            .await?;
        }

        sqlx::query(
            "DELETE FROM github_duplicate_jobs WHERE source_issue_id = $1 AND lease_id = $2",
        )
        .bind(job.source_issue_id)
        .bind(job.lease_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(destination)
    }
}

async fn ensure_destination(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    source: IssueId,
    repository: &str,
    issue: &GithubIssue,
) -> Result<IssueId> {
    let existing = sqlx::query(
        "WITH RECURSIVE chain AS (
            SELECT id, merged_into, 0 AS depth FROM issues
            WHERE state <> 'rejected' AND
                (github_issue_id = $1 OR
                 (lower(github_repository) = lower($2) AND github_number = $3))
            UNION ALL
            SELECT issues.id, issues.merged_into, chain.depth + 1
            FROM chain JOIN issues ON issues.id = chain.merged_into
            WHERE chain.depth < 16
         )
         SELECT id FROM chain WHERE merged_into IS NULL
         ORDER BY depth DESC LIMIT 1",
    )
    .bind(issue.id)
    .bind(repository)
    .bind(issue.number)
    .fetch_optional(&mut **transaction)
    .await?;

    if let Some(existing) = existing {
        let issue_id: IssueId = existing.get("id");
        if issue_id == source {
            return Err(AppError::Conflict(
                "Duplicate target points back to the source issue",
            ));
        }
        sqlx::query(
            "UPDATE issues
             SET title = $2, description = $3, github_url = $4,
                 github_repository = $5, github_number = $6,
                 github_issue_id = $7, github_state = $8,
                 github_checked_at = now(), github_updated_at = $9,
                 state = CASE WHEN $8 = 'closed' THEN 'resolved' ELSE 'unresolved' END,
                 resolved_at = CASE WHEN $8 = 'closed'
                    THEN COALESCE(resolved_at, now()) ELSE NULL END,
                 updated_at = now()
             WHERE id = $1 AND
                 (github_updated_at IS NULL OR github_updated_at <= $9)",
        )
        .bind(issue_id)
        .bind(&issue.title)
        .bind(issue.body.as_deref().unwrap_or_default())
        .bind(&issue.html_url)
        .bind(repository)
        .bind(issue.number)
        .bind(issue.id)
        .bind(issue.state.as_str())
        .bind(issue.updated_at)
        .execute(&mut **transaction)
        .await?;
        return Ok(issue_id);
    }

    let issue_id = IssueId::new();
    let description = issue.body.as_deref().unwrap_or_default();
    let state = if issue.state.as_str() == "closed" {
        "resolved"
    } else {
        "unresolved"
    };
    let resolved_at: Option<DateTime<Utc>> = (state == "resolved").then(Utc::now);
    sqlx::query(
        "INSERT INTO issues (
            id, title, description, github_number, github_url,
            github_repository, github_issue_id, github_state,
            github_checked_at, github_updated_at, state, resolved_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), $9, $10, $11)",
    )
    .bind(issue_id)
    .bind(&issue.title)
    .bind(description)
    .bind(issue.number)
    .bind(&issue.html_url)
    .bind(repository)
    .bind(issue.id)
    .bind(issue.state.as_str())
    .bind(issue.updated_at)
    .bind(state)
    .bind(resolved_at)
    .execute(&mut **transaction)
    .await?;

    sqlx::query(
        "INSERT INTO audit_events (action, entity_id, details)
         VALUES ('issue.create', $1, $2)",
    )
    .bind(issue_id.0)
    .bind(serde_json::json!({ "source": "github_duplicate", "github_number": issue.number }))
    .execute(&mut **transaction)
    .await?;
    Ok(issue_id)
}
