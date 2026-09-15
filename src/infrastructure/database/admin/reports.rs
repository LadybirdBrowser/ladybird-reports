use sqlx::Row;

use crate::{
    domain::{AttachmentId, IssueId, ReportId},
    error::{AppError, Result},
    infrastructure::database::AdminDatabase,
};

use super::{
    AuditEvent, ReportDetails, ReportQuery, ReportRecord, ReportSearchResult, ReportSummary,
    StoredAttachment, StoredDiagnosticField,
};

impl AdminDatabase {
    pub async fn attachment(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<Option<StoredAttachment>> {
        let row = sqlx::query(
            "SELECT
                attachments.id,
                attachments.name,
                attachments.media_type,
                attachments.size,
                attachments.sha256,
                attachments.storage_key
             FROM attachments
             JOIN reports ON reports.id = attachments.report_id
             WHERE attachments.id = $1
                AND attachments.deleted_at IS NULL
                AND reports.deleted_at IS NULL
                AND reports.storage_state = 'ready'",
        )
        .bind(attachment_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| StoredAttachment {
            id: row.get("id"),
            name: row.get("name"),
            media_type: row.get("media_type"),
            size: row.get("size"),
            sha256: row.get("sha256"),
            storage_key: row.get("storage_key"),
        }))
    }

    pub async fn list_reports(&self, query: &ReportQuery) -> Result<Vec<ReportSummary>> {
        let rows = sqlx::query(
            "SELECT
                reports.id,
                reports.client_version,
                reports.build,
                reports.issue_id,
                reports.created_at
             FROM reports
             WHERE reports.deleted_at IS NULL
                AND reports.storage_state = 'ready'
                AND (
                    $1 = 'all'
                    OR ($1 = 'assigned' AND reports.issue_id IS NOT NULL)
                    OR ($1 = 'triage' AND reports.issue_id IS NULL)
                )
                AND ($2 = '' OR reports.build = $2)
                AND ($3 = '' OR EXISTS (
                    SELECT 1
                    FROM report_fields
                    WHERE report_fields.report_id = reports.id
                        AND report_fields.key = 'platform'
                        AND report_fields.value = to_jsonb($3::text)
                ))
                AND ($4 = '' OR reports.kind = $4)
                AND ($5::uuid IS NULL OR reports.issue_id = $5)
                AND ($6::date IS NULL OR reports.created_at >= $6::date)
                AND ($7::date IS NULL OR reports.created_at < $7::date + interval '1 day')
                AND ($8::timestamptz IS NULL OR reports.created_at < $8)
             ORDER BY reports.created_at DESC
             LIMIT 100",
        )
        .bind(query.assignment.as_str())
        .bind(&query.build)
        .bind(&query.platform)
        .bind(&query.kind)
        .bind(query.issue_id)
        .bind(query.since)
        .bind(query.until)
        .bind(query.before)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| ReportSummary {
                id: row.get("id"),
                client_version: row.get("client_version"),
                build: row.get("build"),
                issue_id: row.get("issue_id"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    pub async fn search_reports(&self, search: &str) -> Result<Vec<ReportSearchResult>> {
        let rows = sqlx::query(
            "SELECT
                reports.id,
                reports.kind,
                reports.client_version,
                reports.build,
                issues.title AS issue_title,
                reports.created_at
             FROM reports
             LEFT JOIN issues ON issues.id = reports.issue_id
             WHERE reports.deleted_at IS NULL
                AND reports.storage_state = 'ready'
                AND (
                    $1 = ''
                    OR position(lower($1) in reports.id::text) > 0
                    OR position(lower($1) in lower(reports.kind)) > 0
                    OR position(lower($1) in lower(reports.client_version)) > 0
                    OR position(lower($1) in lower(reports.build)) > 0
                    OR position(lower($1) in lower(COALESCE(issues.title, ''))) > 0
                )
             ORDER BY
                CASE
                    WHEN $1 <> '' AND reports.id::text LIKE lower($1) || '%' THEN 0
                    ELSE 1
                END,
                reports.issue_id IS NOT NULL,
                reports.created_at DESC,
                reports.id DESC
             LIMIT 20",
        )
        .bind(search)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| ReportSearchResult {
                id: row.get("id"),
                kind: row.get("kind"),
                client_version: row.get("client_version"),
                build: row.get("build"),
                issue_title: row.get("issue_title"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    pub async fn report_details(&self, report_id: ReportId) -> Result<Option<ReportDetails>> {
        let report = self.find_report(report_id).await?;

        let Some(report) = report else {
            return Ok(None);
        };

        let fields = self.report_fields(report_id).await?;
        let attachments = self.report_attachments(report_id).await?;
        let events = self.audit_events(report_id.0).await?;

        Ok(Some(ReportDetails {
            report,
            fields,
            attachments,
            events,
        }))
    }

    async fn find_report(&self, report_id: ReportId) -> Result<Option<ReportRecord>> {
        let row = sqlx::query(
            "SELECT
                id,
                submission_id,
                manifest_digest,
                kind,
                client_version,
                build,
                issue_id,
                source_client_key IS NOT NULL AS has_submission_source,
                EXISTS (
                    SELECT 1
                    FROM source_rate_limits
                    WHERE source_rate_limits.client_key = reports.source_client_key
                        AND source_rate_limits.lifted_at IS NULL
                        AND (
                            source_rate_limits.expires_at IS NULL
                            OR source_rate_limits.expires_at > now()
                        )
                ) AS submission_source_is_blocked,
                created_at,
                expires_at
             FROM reports
             WHERE id = $1
                AND deleted_at IS NULL
                AND storage_state = 'ready'",
        )
        .bind(report_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| ReportRecord {
            id: row.get("id"),
            submission_id: row.get("submission_id"),
            manifest_digest: row.get("manifest_digest"),
            kind: row.get("kind"),
            client_version: row.get("client_version"),
            build: row.get("build"),
            issue_id: row.get("issue_id"),
            has_submission_source: row.get("has_submission_source"),
            submission_source_is_blocked: row.get("submission_source_is_blocked"),
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
        }))
    }

    async fn report_fields(&self, report_id: ReportId) -> Result<Vec<StoredDiagnosticField>> {
        let rows = sqlx::query(
            "SELECT
                report_fields.key,
                report_fields.kind,
                report_fields.value,
                report_fields.recognized_at_submission,
                field_definitions.label AS current_label,
                field_definitions.position AS current_position
             FROM report_fields
             LEFT JOIN field_definitions
                ON field_definitions.key = report_fields.key
                AND field_definitions.kind = report_fields.kind
             WHERE report_fields.report_id = $1
             ORDER BY field_definitions.position NULLS LAST, report_fields.key",
        )
        .bind(report_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| StoredDiagnosticField {
                key: row.get("key"),
                kind: row.get("kind"),
                value: row.get("value"),
                recognized_at_submission: row.get("recognized_at_submission"),
                current_label: row.get("current_label"),
                current_position: row.get("current_position"),
            })
            .collect())
    }

    async fn report_attachments(&self, report_id: ReportId) -> Result<Vec<StoredAttachment>> {
        let rows = sqlx::query(
            "SELECT id, name, media_type, size, sha256, storage_key
             FROM attachments
             WHERE report_id = $1 AND deleted_at IS NULL
             ORDER BY created_at, id",
        )
        .bind(report_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| StoredAttachment {
                id: row.get("id"),
                name: row.get("name"),
                media_type: row.get("media_type"),
                size: row.get("size"),
                sha256: row.get("sha256"),
                storage_key: row.get("storage_key"),
            })
            .collect())
    }

    pub async fn assign_reports(
        &self,
        report_ids: &[ReportId],
        issue_id: Option<IssueId>,
        actor: i64,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await?;

        // Human issue operations share one lock. This prevents an assignment from
        // racing with an issue merge while keeping ingestion fully independent.
        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        if let Some(issue_id) = issue_id {
            let assignable: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM issues WHERE id = $1 AND merged_into IS NULL
                )",
            )
            .bind(issue_id)
            .fetch_one(&mut *transaction)
            .await?;

            if !assignable {
                return Err(AppError::InvalidRequest(
                    "Issue does not exist or has been merged",
                ));
            }
        }

        for report_id in report_ids {
            let previous_issue: Option<IssueId> = sqlx::query_scalar(
                "SELECT issue_id
                 FROM reports
                 WHERE id = $1 AND deleted_at IS NULL AND storage_state = 'ready'",
            )
            .bind(report_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(AppError::NotFound("Report not found"))?;

            sqlx::query(
                "UPDATE reports
                 SET
                    issue_id = $2,
                    assigned_at = CASE WHEN $2::uuid IS NULL THEN NULL ELSE now() END,
                    updated_at = now()
                 WHERE id = $1",
            )
            .bind(report_id)
            .bind(issue_id)
            .execute(&mut *transaction)
            .await?;

            sqlx::query(
                "INSERT INTO audit_events (actor, action, entity_id, details)
                 VALUES ($1, 'report.assignment', $2, $3)",
            )
            .bind(actor)
            .bind(report_id.0)
            .bind(serde_json::json!({
                "from": previous_issue,
                "to": issue_id,
            }))
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(())
    }

    pub async fn block_report_source(&self, report_id: ReportId, actor: i64) -> Result<()> {
        let mut transaction = self.pool.begin().await?;
        let client_key: Option<String> = sqlx::query_scalar(
            "SELECT source_client_key
             FROM reports
             WHERE id = $1 AND deleted_at IS NULL AND storage_state = 'ready'",
        )
        .bind(report_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("Report not found"))?;
        let client_key = client_key.ok_or(AppError::InvalidRequest(
            "This report has no submission source identifier",
        ))?;

        sqlx::query(
            "INSERT INTO source_rate_limits (
                client_key,
                source_report_id,
                imposed_by,
                expires_at
             )
             VALUES ($1, $2, $3, NULL)
             ON CONFLICT (client_key) DO UPDATE SET
                source_report_id = excluded.source_report_id,
                imposed_by = excluded.imposed_by,
                imposed_at = now(),
                expires_at = NULL,
                lifted_by = NULL,
                lifted_at = NULL",
        )
        .bind(&client_key)
        .bind(report_id)
        .bind(actor)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id)
             VALUES ($1, 'report.source_blocked', $2)",
        )
        .bind(actor)
        .bind(report_id.0)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn unblock_report_source(&self, report_id: ReportId, actor: i64) -> Result<()> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE source_rate_limits
             SET lifted_by = $2, lifted_at = now()
             WHERE client_key = (
                SELECT source_client_key FROM reports WHERE id = $1
             )
                AND lifted_at IS NULL
                AND (expires_at IS NULL OR expires_at > now())",
        )
        .bind(report_id)
        .bind(actor)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() != 1 {
            return Err(AppError::InvalidRequest("Submission source is not blocked"));
        }

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id)
             VALUES ($1, 'report.source_unblocked', $2)",
        )
        .bind(actor)
        .bind(report_id.0)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    pub(super) async fn audit_events(&self, entity_id: uuid::Uuid) -> Result<Vec<AuditEvent>> {
        let rows = sqlx::query(
            "SELECT
                audit_events.action,
                maintainers.login AS actor_login,
                audit_events.details,
                audit_events.created_at
             FROM audit_events
             LEFT JOIN maintainers ON maintainers.github_id = audit_events.actor
             WHERE audit_events.entity_id = $1
             ORDER BY audit_events.id DESC
             LIMIT 100",
        )
        .bind(entity_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| AuditEvent {
                action: row.get("action"),
                actor_login: row.get("actor_login"),
                details: row.get("details"),
                created_at: row.get("created_at"),
            })
            .collect())
    }
}
