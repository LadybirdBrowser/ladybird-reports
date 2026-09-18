use sqlx::Row;

use crate::{
    domain::{AttachmentId, ReportId},
    error::Result,
};

use super::AdminDatabase;

pub struct PendingFailureReason {
    pub attachment_id: AttachmentId,
    pub report_id: ReportId,
    pub storage_key: String,
}

impl AdminDatabase {
    pub async fn pending_failure_reasons(&self) -> Result<Vec<PendingFailureReason>> {
        let rows = sqlx::query(
            "SELECT attachments.id, attachments.report_id, attachments.storage_key
             FROM attachments
             JOIN reports ON reports.id = attachments.report_id
             WHERE attachments.failure_reason_processed_at IS NULL
                AND attachments.name = 'crash-diagnostics.txt'
                AND attachments.media_type = 'text/plain'
                AND reports.storage_state = 'ready'
             ORDER BY attachments.created_at
             LIMIT 50",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| PendingFailureReason {
                attachment_id: row.get("id"),
                report_id: row.get("report_id"),
                storage_key: row.get("storage_key"),
            })
            .collect())
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
