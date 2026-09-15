use sqlx::{Postgres, QueryBuilder, Row};

use crate::{
    domain::{AttachmentId, IssueId, ReportId},
    error::{AppError, Result},
    infrastructure::database::AdminDatabase,
};

use super::{
    AuditEvent, BlockReportSourceOutcome, ReportDetails, ReportQuery, ReportRecord,
    ReportSearchResult, ReportSummary, StoredAttachment, StoredDiagnosticField,
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
        let mut sql = QueryBuilder::<Postgres>::new(
            "SELECT
                reports.id,
                reports.client_version,
                reports.build,
                reports.issue_id,
                reports.created_at
             FROM reports
             WHERE reports.deleted_at IS NULL
                AND reports.storage_state = 'ready'",
        );

        for term in &query.search.terms {
            push_text_search(&mut sql, term);
        }

        for qualifier in &query.search.qualifiers {
            push_qualified_search(&mut sql, &qualifier.key, &qualifier.value);
        }

        sql.push(" AND (")
            .push_bind(query.issue_id)
            .push("::uuid IS NULL OR reports.issue_id = ")
            .push_bind(query.issue_id)
            .push(") AND (")
            .push_bind(query.since)
            .push("::date IS NULL OR reports.created_at >= ")
            .push_bind(query.since)
            .push("::date) AND (")
            .push_bind(query.until)
            .push("::date IS NULL OR reports.created_at < ")
            .push_bind(query.until)
            .push("::date + interval '1 day') AND (")
            .push_bind(query.before)
            .push("::timestamptz IS NULL OR reports.created_at < ")
            .push_bind(query.before)
            .push(") ORDER BY reports.created_at DESC LIMIT 100");

        let rows = sql.build().fetch_all(&self.pool).await?;

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
                host(source_ip) AS source_ip,
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
            source_ip: row.get("source_ip"),
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

    pub async fn block_report_source(
        &self,
        report_id: ReportId,
        actor: i64,
        remove_triage_reports: bool,
    ) -> Result<BlockReportSourceOutcome> {
        let mut transaction = self.pool.begin().await?;
        let client_key: Option<String> = sqlx::query_scalar(
            "SELECT source_client_key
             FROM reports
             WHERE id = $1 AND deleted_at IS NULL AND storage_state = 'ready'
             FOR UPDATE",
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

        let removed_report_ids = if remove_triage_reports {
            sqlx::query_scalar::<_, ReportId>(
                "UPDATE reports
                 SET deleted_at = now(), updated_at = now()
                 WHERE source_client_key = $1
                    AND issue_id IS NULL
                    AND deleted_at IS NULL
                    AND storage_state = 'ready'
                 RETURNING id",
            )
            .bind(&client_key)
            .fetch_all(&mut *transaction)
            .await?
        } else {
            Vec::new()
        };

        if !removed_report_ids.is_empty() {
            let removed_ids = removed_report_ids
                .iter()
                .map(|report_id| report_id.0)
                .collect::<Vec<_>>();
            sqlx::query(
                "UPDATE attachments
                 SET deleted_at = COALESCE(deleted_at, now())
                 WHERE report_id = ANY($1)",
            )
            .bind(&removed_ids)
            .execute(&mut *transaction)
            .await?;
        }

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id, details)
             VALUES (
                $1,
                'report.source_blocked',
                $2,
                jsonb_build_object('triage_reports_removed', $3::bigint)
             )",
        )
        .bind(actor)
        .bind(report_id.0)
        .bind(removed_report_ids.len() as i64)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(BlockReportSourceOutcome {
            removed_triage_reports: removed_report_ids.len() as u64,
            current_report_removed: removed_report_ids.contains(&report_id),
        })
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

fn push_text_search(sql: &mut QueryBuilder<'_, Postgres>, term: &str) {
    sql.push(" AND (position(lower(")
        .push_bind(term.to_owned())
        .push(") in lower(reports.id::text)) > 0 OR position(lower(")
        .push_bind(term.to_owned())
        .push(") in lower(reports.kind)) > 0 OR position(lower(")
        .push_bind(term.to_owned())
        .push(") in lower(reports.client_version)) > 0 OR position(lower(")
        .push_bind(term.to_owned())
        .push(
            ") in lower(reports.build)) > 0 OR EXISTS (
            SELECT 1 FROM report_fields
            WHERE report_fields.report_id = reports.id
                AND position(lower(",
        )
        .push_bind(term.to_owned())
        .push(") in lower(report_fields.value #>> '{}')) > 0))");
}

fn push_qualified_search(sql: &mut QueryBuilder<'_, Postgres>, key: &str, value: &str) {
    match key {
        "state" => match value.to_ascii_lowercase().as_str() {
            "triage" => {
                sql.push(" AND reports.issue_id IS NULL");
            }
            "assigned" => {
                sql.push(" AND reports.issue_id IS NOT NULL");
            }
            "all" => {}
            _ => unreachable!("report state qualifiers are validated while parsing"),
        },
        "kind" => push_report_column_filter(sql, "reports.kind", value),
        "version" | "client_version" => {
            push_report_column_filter(sql, "reports.client_version", value);
        }
        "build" => push_report_column_filter(sql, "reports.build", value),
        "id" | "report" => push_report_column_filter(sql, "reports.id::text", value),
        "ip" | "source_ip" => push_report_column_filter(sql, "host(reports.source_ip)", value),
        _ => {
            sql.push(
                " AND EXISTS (
                    SELECT 1 FROM report_fields
                    WHERE report_fields.report_id = reports.id
                        AND lower(report_fields.key) = lower(",
            )
            .push_bind(key.to_owned())
            .push(") AND position(lower(")
            .push_bind(value.to_owned())
            .push(") in lower(report_fields.value #>> '{}')) > 0)");
        }
    }
}

fn push_report_column_filter(sql: &mut QueryBuilder<'_, Postgres>, column: &str, value: &str) {
    sql.push(" AND lower(")
        .push(column)
        .push(") = lower(")
        .push_bind(value.to_owned())
        .push(")");
}
