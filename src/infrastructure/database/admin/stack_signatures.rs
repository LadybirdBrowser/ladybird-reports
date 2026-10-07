use std::time::Duration;

use sqlx::{Row, postgres::PgListener};

use crate::{
    domain::{
        AuditAction, IssueId, ReportId, ReportKind, ReportState, STACK_SIGNATURE_VERSION,
        StackSignatureStatus, parse_stack_trace, stack_fingerprint,
    },
    error::Result,
    infrastructure::database::AdminDatabase,
};

use super::{PotentialIssueMatch, ReportSummary, insert_audit_event};

/// Stack traces indexed per batch.
const STACK_INDEX_BATCH_SIZE: usize = 50;

/// Notified when stack traces may need indexing: a report became ready (the
/// payload is its id) or a field definition changed (an empty payload).
const STACK_INDEX_CHANNEL: &str = "ladybird_reports_stack_index";

impl AdminDatabase {
    pub async fn potential_issue_matches(
        &self,
        report_id: ReportId,
    ) -> Result<Vec<PotentialIssueMatch>> {
        let rows = sqlx::query(
            "SELECT issues.id, issues.title, issues.github_state
             FROM report_stack_signatures AS source
             JOIN reports AS incoming ON incoming.id = source.report_id
             JOIN report_stack_signatures AS linked
                ON linked.fingerprint = source.fingerprint
                AND linked.algorithm_version = source.algorithm_version
             JOIN reports AS linked_report
                ON linked_report.id = linked.report_id
                AND linked_report.kind = incoming.kind
             JOIN issues ON issues.id = linked_report.issue_id
             WHERE source.report_id = $1
                AND source.algorithm_version = $2
                AND source.fingerprint IS NOT NULL
                AND incoming.issue_id IS NULL
                AND incoming.storage_state = 'ready'
                AND linked_report.storage_state = 'ready'
                AND issues.merged_into IS NULL
                AND issues.state IN ('unresolved', 'needs_attention')
                AND issues.resolved_at IS NULL
             GROUP BY issues.id
             ORDER BY count(DISTINCT linked_report.id) DESC, issues.updated_at DESC
             LIMIT 5",
        )
        .bind(report_id)
        .bind(STACK_SIGNATURE_VERSION)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| PotentialIssueMatch {
                id: row.get("id"),
                title: row.get("title"),
                github_state: row.get("github_state"),
            })
            .collect())
    }

    pub async fn issue_signature_matches(&self, issue_id: IssueId) -> Result<Vec<ReportSummary>> {
        let mut reports: Vec<ReportSummary> = sqlx::query_as(
            "SELECT reports.id, reports.kind, reports.client_version,
                    reports.state, reports.created_at
             FROM reports
             WHERE reports.issue_id IS NULL
                AND reports.state = 'triage'
                AND reports.storage_state = 'ready'
                AND EXISTS (
                    SELECT 1
                    FROM report_stack_signatures AS candidate
                    JOIN report_stack_signatures AS linked_signature
                        ON linked_signature.fingerprint = candidate.fingerprint
                        AND linked_signature.algorithm_version = candidate.algorithm_version
                    JOIN reports AS linked
                        ON linked.id = linked_signature.report_id
                    WHERE candidate.report_id = reports.id
                        AND candidate.algorithm_version = $2
                        AND candidate.fingerprint IS NOT NULL
                        AND linked.issue_id = $1
                        AND linked.kind = reports.kind
                        AND linked.storage_state = 'ready'
                )
             ORDER BY reports.created_at DESC, reports.id DESC
             LIMIT 20",
        )
        .bind(issue_id)
        .bind(STACK_SIGNATURE_VERSION)
        .fetch_all(&self.pool)
        .await?;
        self.populate_report_titles(&mut reports).await?;
        Ok(reports)
    }

    /// Starts listening for indexing work. Subscribe before the first scan: work
    /// that appears while that scan runs then still has a notification waiting.
    pub async fn listen_for_stack_index_work(&self) -> Result<PgListener> {
        let mut listener = PgListener::connect_with(&self.pool).await?;
        listener.listen(STACK_INDEX_CHANNEL).await?;

        Ok(listener)
    }

    /// Indexes every pending stack trace, a batch at a time, pausing between full
    /// batches so a large backlog does not monopolise the database. Returns how
    /// many traces it indexed.
    pub async fn drain_pending_stack_traces(&self, pause: Duration) -> Result<usize> {
        let mut total = 0;

        loop {
            let count = self.index_pending_stack_traces().await?;
            total += count;

            if count < STACK_INDEX_BATCH_SIZE {
                return Ok(total);
            }

            tokio::time::sleep(pause).await;
        }
    }

    /// Rebuild a bounded batch. Old signatures are replaced when the algorithm changes.
    /// A full batch (`STACK_INDEX_BATCH_SIZE`) means more may be waiting.
    pub async fn index_pending_stack_traces(&self) -> Result<usize> {
        let pending = self
            .stack_traces_to_index(None, STACK_INDEX_BATCH_SIZE as i64)
            .await?;
        let count = pending.len();
        self.store_stack_signatures(pending).await?;
        Ok(count)
    }

    pub async fn index_report_stack_traces(&self, report_id: ReportId) -> Result<()> {
        let pending = self.stack_traces_to_index(Some(report_id), 64).await?;
        self.store_stack_signatures(pending).await
    }

    async fn stack_traces_to_index(
        &self,
        report_id: Option<ReportId>,
        limit: i64,
    ) -> Result<Vec<PendingStackTrace>> {
        let rows = sqlx::query(
            "SELECT reports.id AS report_id, reports.kind, reports.auto_match_eligible,
                    traces.text,
                    signal.value #>> '{}' AS signal,
                    process.value #>> '{}' AS process
             FROM report_stack_traces AS traces
             JOIN reports ON reports.id = traces.report_id
             LEFT JOIN report_fields AS signal
                ON signal.report_id = reports.id AND signal.key = 'signal'
             LEFT JOIN report_fields AS process
                ON process.report_id = reports.id AND process.key = 'process'
             LEFT JOIN report_stack_signatures AS signatures
                ON signatures.report_id = reports.id
             WHERE reports.storage_state = 'ready'
                AND ($1::uuid IS NULL OR reports.id = $1)
                AND (signatures.report_id IS NULL
                    OR signatures.algorithm_version <> $2
                    OR signatures.matched_at IS NULL)
             ORDER BY reports.created_at, reports.id
             LIMIT $3",
        )
        .bind(report_id)
        .bind(STACK_SIGNATURE_VERSION)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| PendingStackTrace {
                report_id: row.get("report_id"),
                kind: row.get("kind"),
                text: row.get("text"),
                signal: row.get("signal"),
                process: row.get("process"),
                auto_match_eligible: row.get("auto_match_eligible"),
            })
            .collect())
    }

    async fn store_stack_signatures(&self, pending: Vec<PendingStackTrace>) -> Result<()> {
        for trace in pending {
            let parsed = parse_stack_trace(&trace.text);
            let fingerprint = stack_fingerprint(
                trace.kind,
                trace.process.as_deref(),
                trace.signal.as_deref(),
                &parsed.frame_keys,
            );
            let status = if fingerprint.is_some() {
                StackSignatureStatus::Parsed
            } else {
                StackSignatureStatus::Insufficient
            };

            let mut transaction = self.pool.begin().await?;

            // Share the issue-operation lock so a merge or hide cannot race with
            // selecting the issue for an incoming report. It also serializes
            // matching reports indexed by two admin instances.
            sqlx::query("SELECT pg_advisory_xact_lock(891125)")
                .execute(&mut *transaction)
                .await?;

            // A report arrives with its signature; only its matching is left.
            let matched: bool = sqlx::query_scalar(
                "SELECT COALESCE(
                    (SELECT matched_at IS NOT NULL
                     FROM report_stack_signatures WHERE report_id = $1),
                    false)",
            )
            .bind(trace.report_id)
            .fetch_one(&mut *transaction)
            .await?;

            sqlx::query(
                "INSERT INTO report_stack_signatures
                    (report_id, algorithm_version, status, fingerprint, frame_keys)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (report_id) DO UPDATE SET
                    algorithm_version = excluded.algorithm_version,
                    status = excluded.status,
                    fingerprint = excluded.fingerprint,
                    frame_keys = excluded.frame_keys,
                    indexed_at = now()",
            )
            .bind(trace.report_id)
            .bind(STACK_SIGNATURE_VERSION)
            .bind(status)
            .bind(&fingerprint)
            .bind(parsed.frame_keys)
            .execute(&mut *transaction)
            .await?;

            if !matched {
                if trace.auto_match_eligible
                    && let Some(fingerprint) = fingerprint
                {
                    self.match_indexed_report(&mut transaction, &trace, &fingerprint)
                        .await?;
                }

                sqlx::query(
                    "UPDATE report_stack_signatures SET matched_at = now() WHERE report_id = $1",
                )
                .bind(trace.report_id)
                .execute(&mut *transaction)
                .await?;
            }

            transaction.commit().await?;
        }
        Ok(())
    }

    async fn match_indexed_report(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        trace: &PendingStackTrace,
        fingerprint: &str,
    ) -> Result<()> {
        let issues: Vec<IssueId> = sqlx::query_scalar(
            "SELECT DISTINCT issues.id
             FROM report_stack_signatures AS signatures
             JOIN reports ON reports.id = signatures.report_id
             JOIN issues ON issues.id = reports.issue_id
             WHERE signatures.report_id <> $1
                AND signatures.algorithm_version = $2
                AND signatures.fingerprint = $3
                AND reports.kind = $4
                AND reports.storage_state = 'ready'
                AND reports.state = 'confirmed'
                AND issues.state <> 'rejected'
                AND issues.merged_into IS NULL
                AND issues.resolved_at IS NULL
                AND issues.github_state = 'open'
             LIMIT 2",
        )
        .bind(trace.report_id)
        .bind(STACK_SIGNATURE_VERSION)
        .bind(fingerprint)
        .bind(trace.kind)
        .fetch_all(&mut **transaction)
        .await?;

        if issues.len() == 1 {
            let issue_id = issues[0];
            let previous_state: Option<ReportState> =
                sqlx::query_scalar("SELECT state FROM reports WHERE id = $1 FOR UPDATE")
                    .bind(trace.report_id)
                    .fetch_optional(&mut **transaction)
                    .await?;
            let assigned = sqlx::query(
                "UPDATE reports
                 SET issue_id = $2, state = 'confirmed',
                     updated_at = now()
                 WHERE id = $1
                    AND issue_id IS NULL
                    AND state <> 'rejected'
                    AND storage_state = 'ready'",
            )
            .bind(trace.report_id)
            .bind(issue_id)
            .execute(&mut **transaction)
            .await?
            .rows_affected()
                == 1;

            if assigned {
                insert_audit_event(
                    &mut **transaction,
                    None,
                    AuditAction::ReportUpdateIssue,
                    Some(trace.report_id.0),
                    serde_json::json!({
                        "from": null,
                        "to": issue_id,
                        "source": "stack_signature",
                        "signature": fingerprint,
                    }),
                )
                .await?;

                if let Some(previous_state) =
                    previous_state.filter(|state| *state != ReportState::Confirmed)
                {
                    insert_audit_event(
                        &mut **transaction,
                        None,
                        AuditAction::ReportUpdateState,
                        Some(trace.report_id.0),
                        serde_json::json!({
                            "from": previous_state,
                            "to": ReportState::Confirmed,
                            "source": "stack_signature",
                        }),
                    )
                    .await?;
                }
            }

            // A signature already associated with an issue needs no Discord alert.
            sqlx::query("DELETE FROM discord_report_notifications WHERE report_id = $1")
                .bind(trace.report_id)
                .execute(&mut **transaction)
                .await?;
            return Ok(());
        }

        if issues.len() > 1 {
            tracing::warn!(
                event = "stack_index.ambiguous_issue_match",
                report_id = %trace.report_id,
                signature = fingerprint,
            );
            return Ok(());
        }

        // With no linked issue, only suppress the alert when another report
        // with this signature arrived in the preceding five minutes.
        let recent_duplicate: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1
                FROM report_stack_signatures AS signatures
                JOIN reports ON reports.id = signatures.report_id
                JOIN reports AS incoming ON incoming.id = $1
                WHERE signatures.report_id <> $1
                    AND signatures.algorithm_version = $2
                    AND signatures.fingerprint = $3
                    AND reports.kind = $4
                    AND reports.storage_state = 'ready'
                    AND reports.state <> 'rejected'
                    AND reports.created_at BETWEEN
                        incoming.created_at - interval '5 minutes' AND incoming.created_at
            )",
        )
        .bind(trace.report_id)
        .bind(STACK_SIGNATURE_VERSION)
        .bind(fingerprint)
        .bind(trace.kind)
        .fetch_one(&mut **transaction)
        .await?;

        if recent_duplicate {
            sqlx::query("DELETE FROM discord_report_notifications WHERE report_id = $1")
                .bind(trace.report_id)
                .execute(&mut **transaction)
                .await?;
        }

        Ok(())
    }
}

struct PendingStackTrace {
    report_id: ReportId,
    kind: ReportKind,
    text: String,
    signal: Option<String>,
    process: Option<String>,
    auto_match_eligible: bool,
}
