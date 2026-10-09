use std::{collections::HashMap, sync::Arc};

use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};

use crate::{
    domain::{
        AttachmentId, AttachmentReference, ChallengeClaims, FieldDefinition, FieldValue, ReportId,
        ReportManifest, RuntimeConfiguration, STACK_SIGNATURE_VERSION, StorageState, SubmissionId,
        UploadId, parse_stack_trace, stack_fingerprint, submitted_stack_trace,
    },
    error::{AppError, Result},
};

use super::{ConfigurationCache, connect_pool};

#[derive(Clone)]
pub struct IngestDatabase {
    pool: PgPool,
    configuration_cache: ConfigurationCache,
}

#[derive(Clone, Debug)]
pub struct ReportReceipt {
    pub report_id: ReportId,
    pub digest_matches: bool,
    pub storage_state: StorageState,
    pub staging_id: UploadId,
}

#[derive(Clone, Debug)]
pub enum AcceptReportOutcome {
    Accepted(ReportId),
    Existing(ReportReceipt),
}

pub struct AcceptReportRequest<'a> {
    pub claims: &'a ChallengeClaims,
    pub token_hash: &'a str,
    pub manifest: &'a ReportManifest,
    pub attachment_ids: &'a HashMap<AttachmentReference, AttachmentId>,
    pub upload_id: UploadId,
    pub source_client_key: &'a str,
    pub source_retention_days: u32,
    pub definitions: &'a HashMap<String, FieldDefinition>,
    pub retention_days: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct IngestionSweepResult {
    pub challenges_deleted: i64,
    pub rate_buckets_deleted: i64,
    pub upload_leases_deleted: i64,
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
    client_reference: &'a str,
    name: &'a str,
    media_type: &'static str,
    size: u64,
    sha256: &'a str,
}

impl IngestDatabase {
    pub async fn connect(database_url: &str) -> Result<Self> {
        Ok(Self {
            pool: connect_pool(database_url).await?,
            configuration_cache: ConfigurationCache::resolving_proxy_hosts(),
        })
    }

    pub async fn start_configuration_cache(&self) -> Result<tokio::task::JoinHandle<()>> {
        self.configuration_cache
            .start(&self.pool, "SELECT reporting_runtime_configuration()")
            .await
    }

    pub async fn configuration(&self) -> Result<Arc<RuntimeConfiguration>> {
        if let Some(configuration) = self.configuration_cache.get() {
            return Ok(configuration);
        }

        let value: Value = sqlx::query_scalar("SELECT reporting_runtime_configuration()")
            .fetch_one(&self.pool)
            .await?;

        let mut configuration: RuntimeConfiguration =
            serde_json::from_value(value).map_err(AppError::internal)?;

        configuration.validate()?;

        // Only the running cache resolves DNS names. Without it a name would
        // silently stop being trusted, so refuse to serve that configuration.
        if configuration.trusted_proxy_hosts().next().is_some() {
            return Err(AppError::Unavailable);
        }

        configuration.resolve_trusted_networks(&HashMap::new());
        Ok(Arc::new(configuration))
    }

    pub async fn field_definitions(&self) -> Result<HashMap<String, FieldDefinition>> {
        let definitions: Vec<FieldDefinition> =
            sqlx::query_as("SELECT key, label, kind, position FROM field_definitions")
                .fetch_all(&self.pool)
                .await?;

        Ok(definitions
            .into_iter()
            .map(|definition| (definition.key.clone(), definition))
            .collect())
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
        request: AcceptReportRequest<'_>,
    ) -> Result<AcceptReportOutcome> {
        let report_id = ReportId::new();
        let mut transaction = self.pool.begin().await?;

        let fields = request
            .manifest
            .fields
            .iter()
            .map(|field| StoredField {
                key: &field.key,
                kind: field.value.kind().as_str(),
                value: field.value.json_value(),
                recognized: request.definitions.contains_key(&field.key),
            })
            .collect::<Vec<_>>();

        let attachments = request
            .manifest
            .attachments
            .iter()
            .map(|attachment| StoredAttachment {
                id: request.attachment_ids[&attachment.id],
                client_reference: attachment.id.as_str(),
                name: &attachment.name,
                media_type: attachment.media_type.as_str(),
                size: attachment.size,
                sha256: &attachment.sha256,
            })
            .collect::<Vec<_>>();

        let row = sqlx::query(
            "SELECT * FROM accept_report(
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::integer, $12, $13
            )",
        )
        .bind(request.claims.id)
        .bind(request.token_hash)
        .bind(&request.claims.manifest_digest)
        .bind(report_id)
        .bind(request.manifest.submission_id)
        .bind(request.manifest.kind.as_str())
        .bind(&request.manifest.client_version)
        .bind(&request.manifest.build)
        .bind(request.upload_id)
        .bind(request.source_client_key)
        .bind(request.source_retention_days as i32)
        .bind(serde_json::to_value(fields).expect("stored fields are serializable"))
        .bind(serde_json::to_value(attachments).expect("stored attachments are serializable"))
        .fetch_one(&mut *transaction)
        .await?;

        let outcome: String = row.get("outcome");
        let returned_report_id: Option<ReportId> = row.get("report_id");

        match outcome.as_str() {
            "accepted" => {
                let report_id = returned_report_id.expect("accepted report has an ID");
                sqlx::query("SELECT configure_report_retention($1, $2)")
                    .bind(report_id)
                    .bind(request.retention_days as i32)
                    .execute(&mut *transaction)
                    .await?;

                let fields = &request.manifest.fields;
                if let Some(stack) = submitted_stack_trace(fields, request.definitions) {
                    let text_of = |key| {
                        fields.iter().find_map(|field| match &field.value {
                            FieldValue::Text(text) if field.key == key => Some(text.as_str()),
                            _ => None,
                        })
                    };
                    sqlx::query("SELECT accept_stack_signature($1, $2, $3)")
                        .bind(report_id)
                        .bind(STACK_SIGNATURE_VERSION)
                        .bind(stack_fingerprint(
                            request.manifest.kind,
                            text_of("process"),
                            text_of("signal"),
                            text_of("failure_reason"),
                            &parse_stack_trace(stack),
                        ))
                        .execute(&mut *transaction)
                        .await?;
                }
                transaction.commit().await?;

                Ok(AcceptReportOutcome::Accepted(report_id))
            }
            "existing" => {
                transaction.commit().await?;
                let receipt = self
                    .find_receipt(
                        request.manifest.submission_id,
                        &request.claims.manifest_digest,
                    )
                    .await?
                    .expect("existing outcome has a receipt");

                Ok(AcceptReportOutcome::Existing(receipt))
            }
            "submission_conflict" => {
                transaction.rollback().await?;
                Err(AppError::Conflict("Submission ID already used"))
            }
            "challenge_rejected" => {
                transaction.rollback().await?;
                Err(AppError::Conflict(
                    "Challenge expired, invalid, or already consumed",
                ))
            }
            "invalid_payload" => {
                transaction.rollback().await?;
                Err(AppError::InvalidRequest("Invalid report payload"))
            }
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

    pub async fn sweep_expired_state(&self) -> Result<IngestionSweepResult> {
        let row = sqlx::query("SELECT * FROM sweep_expired_ingestion_state()")
            .fetch_one(&self.pool)
            .await?;

        Ok(IngestionSweepResult {
            challenges_deleted: row.get("challenges_deleted"),
            rate_buckets_deleted: row.get("rate_buckets_deleted"),
            upload_leases_deleted: row.get("upload_leases_deleted"),
        })
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
