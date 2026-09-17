use sqlx::Row;

use crate::{domain::ReportId, error::Result, infrastructure::database::AdminDatabase};

#[derive(Debug, Default)]
pub struct AdminSweepResult {
    pub sessions_deleted: u64,
    pub oauth_states_deleted: u64,
    pub submission_sources_deleted: u64,
    pub inactive_source_blocks_deleted: u64,
    pub reports_ready_for_purge: Vec<ReportId>,
}

impl AdminDatabase {
    pub async fn begin_maintenance_sweep(
        &self,
        report_retention_days: u32,
        source_retention_days: u32,
    ) -> Result<AdminSweepResult> {
        let mut transaction = self.pool.begin().await?;

        let sessions_deleted = sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
            .execute(&mut *transaction)
            .await?
            .rows_affected();
        let oauth_states_deleted =
            sqlx::query("DELETE FROM oauth_states WHERE expires_at <= now()")
                .execute(&mut *transaction)
                .await?
                .rows_affected();

        let submission_sources_deleted = sqlx::query(
            "UPDATE reports
             SET source_client_key = NULL,
                 source_client_key_expires_at = NULL
             WHERE source_client_key_expires_at <= now()",
        )
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        let inactive_source_blocks_deleted = sqlx::query(
            "DELETE FROM source_rate_limits
             WHERE (lifted_at IS NOT NULL
                    AND lifted_at <= now() - make_interval(days => $1))
                OR (expires_at IS NOT NULL
                    AND expires_at <= now() - make_interval(days => $1))",
        )
        .bind(source_retention_days as i32)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        sqlx::query(
            "UPDATE reports
             SET expires_at = created_at + make_interval(days => $1)
             WHERE expires_at IS NULL",
        )
        .bind(report_retention_days as i32)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "UPDATE attachments
             SET expires_at = reports.expires_at
             FROM reports
             WHERE attachments.report_id = reports.id
                AND attachments.expires_at IS NULL",
        )
        .execute(&mut *transaction)
        .await?;

        let rows = sqlx::query(
            "SELECT id
             FROM reports
             WHERE expires_at <= now()
             ORDER BY expires_at, id
             LIMIT 100",
        )
        .fetch_all(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(AdminSweepResult {
            sessions_deleted,
            oauth_states_deleted,
            submission_sources_deleted,
            inactive_source_blocks_deleted,
            reports_ready_for_purge: rows.into_iter().map(|row| row.get("id")).collect(),
        })
    }

    pub async fn finish_report_purge(&self, report_id: ReportId) -> Result<bool> {
        let result = sqlx::query("DELETE FROM reports WHERE id = $1 AND expires_at <= now()")
            .bind(report_id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() == 1)
    }
}
