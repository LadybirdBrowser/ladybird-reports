use sqlx::Row;

use crate::{
    domain::{DiscordDeliveryLeaseId, ReportId},
    error::Result,
    infrastructure::database::AdminDatabase,
};

use super::PendingDiscordNotification;

impl AdminDatabase {
    pub async fn claim_discord_notification(
        &self,
        lease_seconds: u64,
    ) -> Result<Option<PendingDiscordNotification>> {
        let mut transaction = self.pool.begin().await?;
        let dispatcher_is_available: bool = sqlx::query_scalar(
            "SELECT
                (lease_id IS NULL OR lease_expires_at <= now())
                AND (paused_until IS NULL OR paused_until <= now())
             FROM discord_delivery_state
             WHERE singleton = true
             FOR UPDATE",
        )
        .fetch_one(&mut *transaction)
        .await?;

        if !dispatcher_is_available {
            transaction.commit().await?;
            return Ok(None);
        }

        let row = sqlx::query(
            "SELECT
                notifications.report_id,
                notifications.attempt_count,
                reports.kind,
                reports.client_version,
                reports.build,
                reports.created_at,
                COALESCE(
                    (
                        SELECT jsonb_object_agg(report_fields.key, report_fields.value)
                        FROM report_fields
                        WHERE report_fields.report_id = reports.id
                    ),
                    '{}'::jsonb
                ) AS fields
             FROM discord_report_notifications AS notifications
             JOIN reports ON reports.id = notifications.report_id
             WHERE notifications.delivered_at IS NULL
                AND reports.storage_state = 'ready'
                AND reports.deleted_at IS NULL
             ORDER BY notifications.created_at, notifications.report_id
             LIMIT 1",
        )
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(row) = row else {
            sqlx::query(
                "UPDATE discord_delivery_state
                 SET lease_id = NULL, leased_report_id = NULL, lease_expires_at = NULL
                 WHERE singleton = true",
            )
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            return Ok(None);
        };

        let report_id = row.get("report_id");
        let lease_id = DiscordDeliveryLeaseId::new();
        sqlx::query(
            "UPDATE discord_delivery_state
             SET
                lease_id = $1,
                leased_report_id = $2,
                lease_expires_at = now() + make_interval(secs => $3)
             WHERE singleton = true",
        )
        .bind(lease_id)
        .bind(report_id)
        .bind(lease_seconds as i32)
        .execute(&mut *transaction)
        .await?;

        let attempt_count: i32 = row.get("attempt_count");
        let notification = PendingDiscordNotification {
            report_id,
            lease_id,
            kind: row.get("kind"),
            client_version: row.get("client_version"),
            build: row.get("build"),
            fields: row.get("fields"),
            created_at: row.get("created_at"),
            attempt_count: attempt_count as u32,
        };

        transaction.commit().await?;
        Ok(Some(notification))
    }

    pub async fn finish_discord_notification(
        &self,
        report_id: ReportId,
        lease_id: DiscordDeliveryLeaseId,
        cooldown_seconds: u64,
    ) -> Result<bool> {
        let mut transaction = self.pool.begin().await?;
        let owns_lease = self
            .lock_discord_delivery(&mut transaction, report_id, lease_id)
            .await?;

        if !owns_lease {
            transaction.commit().await?;
            return Ok(false);
        }

        let delivered = sqlx::query(
            "UPDATE discord_report_notifications
             SET delivered_at = now(), last_failure = NULL, last_response_status = NULL
             WHERE report_id = $1 AND delivered_at IS NULL",
        )
        .bind(report_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;

        sqlx::query(
            "UPDATE discord_delivery_state
             SET
                lease_id = NULL,
                leased_report_id = NULL,
                lease_expires_at = NULL,
                paused_until = CASE
                    WHEN $1 = 0 THEN NULL
                    ELSE now() + make_interval(secs => $1)
                END,
                consecutive_failures = 0
             WHERE singleton = true",
        )
        .bind(cooldown_seconds as i32)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(delivered)
    }

    pub async fn defer_discord_notification(
        &self,
        report_id: ReportId,
        lease_id: DiscordDeliveryLeaseId,
        delay_seconds: u64,
        response_status: Option<u16>,
        failure: &str,
    ) -> Result<bool> {
        let mut transaction = self.pool.begin().await?;
        let owns_lease = self
            .lock_discord_delivery(&mut transaction, report_id, lease_id)
            .await?;

        if !owns_lease {
            transaction.commit().await?;
            return Ok(false);
        }

        let deferred = sqlx::query(
            "UPDATE discord_report_notifications
             SET
                attempt_count = attempt_count + 1,
                last_failure = $2,
                last_response_status = $3
             WHERE report_id = $1 AND delivered_at IS NULL",
        )
        .bind(report_id)
        .bind(failure)
        .bind(response_status.map(|status| status as i16))
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            == 1;

        sqlx::query(
            "UPDATE discord_delivery_state
             SET
                lease_id = NULL,
                leased_report_id = NULL,
                lease_expires_at = NULL,
                paused_until = now() + make_interval(secs => $1),
                consecutive_failures = consecutive_failures + 1
             WHERE singleton = true",
        )
        .bind(delay_seconds as i32)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(deferred)
    }

    async fn lock_discord_delivery(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        report_id: ReportId,
        lease_id: DiscordDeliveryLeaseId,
    ) -> Result<bool> {
        let owns_lease: bool = sqlx::query_scalar(
            "SELECT lease_id = $1 AND leased_report_id = $2
             FROM discord_delivery_state
             WHERE singleton = true
             FOR UPDATE",
        )
        .bind(lease_id)
        .bind(report_id)
        .fetch_one(&mut **transaction)
        .await?;

        Ok(owns_lease)
    }
}
