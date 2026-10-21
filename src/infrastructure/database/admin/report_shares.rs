use chrono::{DateTime, Utc};

use crate::{
    domain::{AuditAction, ReportId},
    error::{AppError, Result},
    infrastructure::database::AdminDatabase,
};

use super::{ReportShareAccess, insert_audit_event};

impl AdminDatabase {
    /// Stores a link that works for 24 hours, given the hash of its token.
    pub async fn create_report_share(
        &self,
        report_id: ReportId,
        actor: i64,
        token_hash: &str,
    ) -> Result<DateTime<Utc>> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM reports WHERE id = $1 AND storage_state = 'ready' FOR SHARE",
        )
        .bind(report_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("Report not found"))?;

        let share_id = uuid::Uuid::now_v7();
        let expires_at: DateTime<Utc> = sqlx::query_scalar(
            "INSERT INTO report_shares (id, report_id, token_hash, created_by, expires_at)
             VALUES ($1, $2, $3, $4, now() + interval '24 hours')
             RETURNING expires_at",
        )
        .bind(share_id)
        .bind(report_id)
        .bind(token_hash)
        .bind(actor)
        .fetch_one(&mut *transaction)
        .await?;

        insert_audit_event(
            &mut *transaction,
            Some(actor),
            AuditAction::ReportShareCreate,
            Some(report_id.0),
            serde_json::json!({ "share_id": share_id, "expires_at": expires_at }),
        )
        .await?;
        transaction.commit().await?;

        Ok(expires_at)
    }

    /// Opens a link by the hash of its token, counting the use. Nothing is returned for a link that does not exist or
    /// has expired, which cannot be told apart.
    pub async fn open_report_share(&self, token_hash: &str) -> Result<Option<ReportShareAccess>> {
        sqlx::query_as(
            "UPDATE report_shares
             SET use_count = use_count + 1, last_used_at = now()
             FROM reports
             WHERE report_shares.token_hash = $1
                AND report_shares.expires_at > now()
                AND reports.id = report_shares.report_id
                AND reports.storage_state = 'ready'
             RETURNING report_shares.report_id, report_shares.expires_at",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }
}
