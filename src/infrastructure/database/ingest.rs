use std::collections::HashMap;

use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};

use crate::{
    domain::{
        AttachmentId, ChallengeClaims, FieldDefinition, FieldKind, ReportId, ReportManifest,
        RuntimeConfiguration, SubmissionId, UploadId,
    },
    error::{AppError, Result},
};

use super::connect_pool;

#[derive(Clone)]
pub struct IngestDatabase {
    pool: PgPool,
}

#[derive(Clone, Debug)]
pub struct ReportReceipt {
    pub report_id: ReportId,
    pub digest_matches: bool,
    pub storage_state: String,
    pub staging_id: UploadId,
}

#[derive(Clone, Debug)]
pub enum AcceptReportOutcome {
    Accepted(ReportId),
    Existing(ReportReceipt),
}

#[derive(Serialize)]
struct StoredField<'a> {
    key: &'a str,
    kind: &'static str,
    value: Value,
    recognized: bool,
}

#[derive(Serialize)]
struct StoredAttachment<'a> {
    id: AttachmentId,
    client_id: AttachmentId,
    name: &'a str,
    media_type: &'static str,
    size: u64,
    sha256: &'a str,
}

impl IngestDatabase {
    pub async fn connect(database_url: &str) -> Result<Self> {
        Ok(Self {
            pool: connect_pool(database_url, 16).await?,
        })
    }

    pub async fn configuration(&self) -> Result<RuntimeConfiguration> {
        let value: Value =
            sqlx::query_scalar("SELECT value FROM runtime_configuration WHERE singleton = true")
                .fetch_one(&self.pool)
                .await?;

        let configuration: RuntimeConfiguration =
            serde_json::from_value(value).map_err(|error| AppError::Internal(error.into()))?;

        configuration.validate()?;
        Ok(configuration)
    }

    pub async fn field_definitions(&self) -> Result<HashMap<String, FieldDefinition>> {
        let rows = sqlx::query(
            "SELECT key, label, kind, position FROM field_definitions ORDER BY position, key",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let key: String = row.get("key");
                let kind: String = row.get("kind");
                let kind = FieldKind::parse(&kind).ok_or_else(|| {
                    AppError::Internal(anyhow::anyhow!("invalid field kind in database"))
                })?;

                let definition = FieldDefinition {
                    key: key.clone(),
                    label: row.get("label"),
                    kind,
                    position: row.get("position"),
                };

                Ok((key, definition))
            })
            .collect()
    }

    pub async fn issue_challenge(&self, claims: &ChallengeClaims, token_hash: &str) -> Result<()> {
        let expires_at = DateTime::from_timestamp(claims.expires_at_unix, 0)
            .ok_or(AppError::InvalidRequest("Invalid challenge expiry"))?;

        sqlx::query("SELECT issue_challenge($1, $2, $3, $4)")
            .bind(claims.id)
            .bind(&claims.manifest_digest)
            .bind(token_hash)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn consume_rate_limit(
        &self,
        key: &str,
        refill_count: u32,
        refill_seconds: u32,
        capacity: u32,
    ) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT consume_rate_limit($1, $2, $3, $4)")
                .bind(key)
                .bind(refill_count as f64)
                .bind(refill_seconds as f64)
                .bind(capacity as f64)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn source_has_active_rate_limit(&self, client_key: &str) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT source_has_active_rate_limit($1)")
                .bind(client_key)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn acquire_upload_lease(
        &self,
        upload_id: UploadId,
        client_key: &str,
        per_client_limit: u32,
        global_limit: u32,
        lifetime_seconds: u64,
    ) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT acquire_upload_lease($1, $2, $3, $4, $5)")
                .bind(upload_id)
                .bind(client_key)
                .bind(per_client_limit as i32)
                .bind(global_limit as i32)
                .bind(lifetime_seconds as i32)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn release_upload_lease(&self, upload_id: UploadId) -> Result<()> {
        sqlx::query("SELECT release_upload_lease($1)")
            .bind(upload_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn find_receipt(
        &self,
        submission_id: SubmissionId,
        digest: &str,
    ) -> Result<Option<ReportReceipt>> {
        let row = sqlx::query(
            "SELECT *
             FROM find_report_receipt($1, $2)
             WHERE report_id IS NOT NULL",
        )
        .bind(submission_id)
        .bind(digest)
        .fetch_optional(&self.pool)
        .await?;

        row.map(receipt_from_row).transpose()
    }

    pub async fn accept_report(
        &self,
        claims: &ChallengeClaims,
        token_hash: &str,
        manifest: &ReportManifest,
        upload_id: UploadId,
        source_client_key: &str,
        definitions: &HashMap<String, FieldDefinition>,
    ) -> Result<AcceptReportOutcome> {
        let report_id = ReportId::new();

        let fields = manifest
            .fields
            .iter()
            .map(|field| StoredField {
                key: &field.key,
                kind: field.value.kind().as_str(),
                value: field.value.json_value(),
                recognized: definitions.contains_key(&field.key),
            })
            .collect::<Vec<_>>();

        let attachments = manifest
            .attachments
            .iter()
            .map(|attachment| StoredAttachment {
                id: AttachmentId::new(),
                client_id: attachment.id,
                name: &attachment.name,
                media_type: attachment.media_type.as_str(),
                size: attachment.size,
                sha256: &attachment.sha256,
            })
            .collect::<Vec<_>>();

        let row = sqlx::query(
            "SELECT * FROM accept_report(
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12
            )",
        )
        .bind(claims.id)
        .bind(token_hash)
        .bind(&claims.manifest_digest)
        .bind(report_id)
        .bind(manifest.submission_id)
        .bind(manifest.kind.as_str())
        .bind(&manifest.client_version)
        .bind(&manifest.build)
        .bind(upload_id)
        .bind(source_client_key)
        .bind(serde_json::to_value(fields).expect("stored fields are serializable"))
        .bind(serde_json::to_value(attachments).expect("stored attachments are serializable"))
        .fetch_one(&self.pool)
        .await?;

        let outcome: String = row.get("outcome");
        let returned_report_id: Option<ReportId> = row.get("report_id");

        match outcome.as_str() {
            "accepted" => Ok(AcceptReportOutcome::Accepted(
                returned_report_id.expect("accepted report has an ID"),
            )),
            "existing" => {
                let receipt = self
                    .find_receipt(manifest.submission_id, &claims.manifest_digest)
                    .await?
                    .expect("existing outcome has a receipt");

                Ok(AcceptReportOutcome::Existing(receipt))
            }
            "submission_conflict" => Err(AppError::Conflict("Submission ID already used")),
            "challenge_rejected" => Err(AppError::Conflict(
                "Challenge expired, invalid, or already consumed",
            )),
            "invalid_payload" => Err(AppError::InvalidRequest("Invalid report payload")),
            _ => Err(AppError::Internal(anyhow::anyhow!(
                "unknown accept_report outcome: {outcome}"
            ))),
        }
    }

    pub async fn mark_storage_ready(
        &self,
        report_id: ReportId,
        upload_id: UploadId,
    ) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT mark_report_storage_ready($1, $2)")
                .bind(report_id)
                .bind(upload_id)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn pending_storage(&self) -> Result<Vec<(ReportId, UploadId)>> {
        let rows = sqlx::query("SELECT * FROM pending_report_storage()")
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| (row.get("report_id"), row.get("staging_id")))
            .collect())
    }

    pub async fn staging_upload_is_referenced(&self, upload_id: UploadId) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT staging_upload_is_referenced($1)")
                .bind(upload_id)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn healthcheck(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }
}

fn receipt_from_row(row: sqlx::postgres::PgRow) -> Result<ReportReceipt> {
    Ok(ReportReceipt {
        report_id: row.try_get("report_id")?,
        digest_matches: row.try_get("digest_matches")?,
        storage_state: row.try_get("storage_state")?,
        staging_id: row.try_get("staging_id")?,
    })
}
