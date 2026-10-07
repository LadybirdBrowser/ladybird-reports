use crate::{
    domain::{AttachmentId, ReportId},
    error::Result,
};

use super::AdminDatabase;

/// Attachments examined per backfill batch. A full batch means more may be waiting.
pub const FAILURE_REASON_BATCH_SIZE: usize = 50;

#[derive(sqlx::FromRow)]
pub struct PendingFailureReason {
    pub attachment_id: AttachmentId,
    pub report_id: ReportId,
    pub storage_key: String,
}

impl AdminDatabase {
    pub async fn pending_failure_reasons(&self) -> Result<Vec<PendingFailureReason>> {
        sqlx::query_as(
            "SELECT attachments.id AS attachment_id, attachments.report_id, attachments.storage_key
             FROM attachments
             JOIN reports ON reports.id = attachments.report_id
             WHERE attachments.failure_reason_processed_at IS NULL
                AND attachments.name = 'crash-diagnostics.txt'
                AND attachments.media_type = 'text/plain'
                AND reports.storage_state = 'ready'
             ORDER BY attachments.created_at
             LIMIT $1",
        )
        .bind(FAILURE_REASON_BATCH_SIZE as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn finish_failure_reason(
        &self,
        attachment: &PendingFailureReason,
        reason: Option<&str>,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await?;

        if let Some(reason) = reason {
            sqlx::query(
                "INSERT INTO report_fields (report_id, key, kind, value, recognized_at_submission)
                 VALUES ($1, 'failure_reason', 'text', to_jsonb($2::text), false)
                 ON CONFLICT (report_id, key) DO NOTHING",
            )
            .bind(attachment.report_id)
            .bind(reason)
            .execute(&mut *transaction)
            .await?;
        }

        sqlx::query(
            "UPDATE attachments
             SET failure_reason_processed_at = now()
             WHERE id = $1",
        )
        .bind(attachment.attachment_id)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }
}
