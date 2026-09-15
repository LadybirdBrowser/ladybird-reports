use std::{collections::HashMap, net::IpAddr, sync::Arc};

use chrono::{Duration, Utc};

use crate::{
    domain::{
        ChallengeClaims, ChallengeId, FieldDefinition, ReportId, ReportManifest,
        RuntimeConfiguration, UploadId, is_sha256_hex, proof_is_valid, sha256_hex, sign_challenge,
        verify_challenge,
    },
    error::{AppError, Result},
    infrastructure::{
        attachments::{FileAttachmentStore, StagingUpload},
        database::{AcceptReportOutcome, AcceptReportRequest, IngestDatabase, ReportReceipt},
    },
};

#[derive(Clone)]
pub struct ReportIngestionService {
    database: IngestDatabase,
    attachments: FileAttachmentStore,
    proof_key: Arc<[u8; 32]>,
}

pub struct PreparedSubmission {
    pub manifest: ReportManifest,
    pub upload_id: UploadId,
    pub staging: StagingUpload,
    claims: ChallengeClaims,
    token_hash: String,
    definitions: HashMap<String, FieldDefinition>,
}

pub enum PrepareSubmissionOutcome {
    Ready(Box<PreparedSubmission>),
    Existing(ReportId),
}

impl ReportIngestionService {
    pub fn new(
        database: IngestDatabase,
        attachments: FileAttachmentStore,
        proof_key: [u8; 32],
    ) -> Self {
        Self {
            database,
            attachments,
            proof_key: Arc::new(proof_key),
        }
    }

    pub async fn issue_challenge(&self, manifest_digest: &str) -> Result<IssuedChallenge> {
        if !is_sha256_hex(manifest_digest) {
            return Err(AppError::InvalidRequest("Invalid manifest digest"));
        }

        let configuration = self.database.configuration().await?;
        let proof_configuration = &configuration.proof_of_work;
        let expiry =
            Utc::now() + Duration::seconds(proof_configuration.challenge_lifetime_seconds as i64);

        let claims = ChallengeClaims {
            version: 1,
            id: ChallengeId::new(),
            manifest_digest: manifest_digest.to_owned(),
            expires_at_unix: expiry.timestamp(),
            expected_work: proof_configuration.expected_work,
        };

        let token = sign_challenge(&self.proof_key, &claims);
        self.database
            .issue_challenge(&claims, &sha256_hex(&token))
            .await?;

        Ok(IssuedChallenge {
            token,
            expires_at_unix: claims.expires_at_unix,
            expected_work: claims.expected_work,
        })
    }

    pub async fn prepare_submission(
        &self,
        challenge_token: &str,
        nonce: u64,
        manifest_bytes: &[u8],
    ) -> Result<PrepareSubmissionOutcome> {
        let claims = verify_challenge(&self.proof_key, challenge_token)?;

        if claims.version != 1 {
            return Err(AppError::InvalidRequest(
                "Unsupported proof-of-work version",
            ));
        }

        if !proof_is_valid(challenge_token, nonce, claims.expected_work) {
            return Err(AppError::InvalidRequest("Invalid proof of work"));
        }

        if sha256_hex(manifest_bytes) != claims.manifest_digest {
            return Err(AppError::InvalidRequest(
                "Manifest digest does not match challenge",
            ));
        }

        let manifest: ReportManifest = serde_json::from_slice(manifest_bytes)
            .map_err(|_| AppError::InvalidRequest("Invalid report manifest"))?;

        if let Some(receipt) = self
            .database
            .find_receipt(manifest.submission_id, &claims.manifest_digest)
            .await?
        {
            return self.handle_existing_receipt(receipt).await;
        }

        if claims.expires_at_unix <= Utc::now().timestamp() {
            return Err(AppError::Conflict("Challenge expired"));
        }

        let configuration = self.database.configuration().await?;
        let definitions = self.database.field_definitions().await?;

        manifest.validate(&definitions, &configuration.limits)?;
        self.ensure_storage_capacity(&configuration)?;

        let upload_id = UploadId::new();
        let staging = self.attachments.begin_staging(upload_id).await?;

        Ok(PrepareSubmissionOutcome::Ready(Box::new(
            PreparedSubmission {
                manifest,
                upload_id,
                staging,
                claims,
                token_hash: sha256_hex(challenge_token),
                definitions,
            },
        )))
    }

    pub async fn accept_submission(
        &self,
        mut prepared: PreparedSubmission,
        source_client_key: &str,
        source_ip: IpAddr,
    ) -> Result<ReportId> {
        let configuration = self.database.configuration().await?;

        prepared
            .staging
            .validate(&prepared.manifest.attachments, &configuration.limits)
            .await?;

        let outcome = self
            .database
            .accept_report(AcceptReportRequest {
                claims: &prepared.claims,
                token_hash: &prepared.token_hash,
                manifest: &prepared.manifest,
                upload_id: prepared.upload_id,
                source_client_key,
                source_ip,
                definitions: &prepared.definitions,
                retention_days: configuration.maintenance.report_retention_days,
            })
            .await?;

        let report_id = match outcome {
            AcceptReportOutcome::Accepted(report_id) => {
                prepared.staging.mark_referenced();
                report_id
            }
            AcceptReportOutcome::Existing(receipt) => {
                if receipt.staging_id != prepared.upload_id {
                    self.attachments.remove_staging(prepared.upload_id).await?;
                } else {
                    prepared.staging.mark_referenced();
                }

                self.recover_receipt(&receipt).await?;
                return Ok(receipt.report_id);
            }
        };

        self.attachments
            .finalize(prepared.upload_id, report_id)
            .await?;

        let marked_ready = self
            .database
            .mark_storage_ready(report_id, prepared.upload_id)
            .await?;

        if !marked_ready {
            return Err(AppError::Internal(anyhow::anyhow!(
                "accepted report could not be marked ready"
            )));
        }

        tracing::info!(
            event = "report.accepted",
            %report_id,
            field_count = prepared.manifest.fields.len(),
            attachment_count = prepared.manifest.attachments.len(),
        );
        Ok(report_id)
    }

    pub async fn abandon_submission(&self, upload_id: UploadId) {
        if let Err(error) = self.attachments.remove_staging(upload_id).await {
            tracing::warn!(?error, %upload_id, "Could not remove abandoned upload staging");
        }
    }

    pub async fn acquire_upload_lease(
        &self,
        upload_id: UploadId,
        client_key: &str,
        configuration: &RuntimeConfiguration,
    ) -> Result<bool> {
        let limits = &configuration.limits;

        self.database
            .acquire_upload_lease(
                upload_id,
                client_key,
                limits.concurrent_uploads_per_ip,
                limits.concurrent_uploads_global,
                limits.upload_timeout_seconds + 30,
            )
            .await
    }

    pub async fn release_upload_lease(&self, upload_id: UploadId) {
        if let Err(error) = self.database.release_upload_lease(upload_id).await {
            tracing::warn!(?error, %upload_id, "Could not release upload lease");
        }
    }

    pub async fn recover_pending_storage(&self) -> Result<()> {
        let pending = self.database.pending_storage().await?;

        if !pending.is_empty() {
            tracing::info!(
                event = "attachments.recovery_started",
                report_count = pending.len()
            );
        }

        for (report_id, staging_id) in pending {
            let receipt = ReportReceipt {
                report_id,
                digest_matches: true,
                storage_state: "pending_files".into(),
                staging_id,
            };

            if let Err(error) = self.recover_receipt(&receipt).await {
                tracing::warn!(
                    event = "attachments.recovery_failed",
                    ?error,
                    %report_id,
                    "Could not recover pending report storage"
                );
            }
        }

        Ok(())
    }

    pub async fn configuration(&self) -> Result<RuntimeConfiguration> {
        self.database.configuration().await
    }

    pub async fn sweep_ingestion_state(&self) -> Result<()> {
        let configuration = self.database.configuration().await?;
        let expired = self.database.sweep_expired_state().await?;
        let older_than = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(
                configuration.maintenance.staging_retention_seconds,
            ))
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("invalid staging cutoff")))?;

        let mut staging_deleted = 0_u64;
        for upload_id in self.attachments.stale_staging_uploads(older_than).await? {
            if !self
                .database
                .staging_upload_is_referenced(upload_id)
                .await?
            {
                self.attachments.remove_staging(upload_id).await?;
                staging_deleted += 1;
            }
        }

        let total_deleted = expired.challenges_deleted
            + expired.rate_buckets_deleted
            + expired.upload_leases_deleted
            + staging_deleted as i64;
        if total_deleted > 0 {
            tracing::info!(
                event = "maintenance.ingestion_sweep_completed",
                challenges_deleted = expired.challenges_deleted,
                rate_buckets_deleted = expired.rate_buckets_deleted,
                upload_leases_deleted = expired.upload_leases_deleted,
                staging_deleted,
            );
        }

        Ok(())
    }

    pub async fn healthcheck(&self) -> Result<()> {
        self.database.healthcheck().await?;
        tokio::fs::metadata(self.attachments.root()).await?;
        Ok(())
    }

    async fn handle_existing_receipt(
        &self,
        receipt: ReportReceipt,
    ) -> Result<PrepareSubmissionOutcome> {
        if !receipt.digest_matches {
            return Err(AppError::Conflict("Submission ID already used"));
        }

        self.recover_receipt(&receipt).await?;
        Ok(PrepareSubmissionOutcome::Existing(receipt.report_id))
    }

    async fn recover_receipt(&self, receipt: &ReportReceipt) -> Result<()> {
        if receipt.storage_state == "ready" {
            return Ok(());
        }

        self.attachments
            .finalize(receipt.staging_id, receipt.report_id)
            .await?;

        self.database
            .mark_storage_ready(receipt.report_id, receipt.staging_id)
            .await?;

        Ok(())
    }

    fn ensure_storage_capacity(&self, configuration: &RuntimeConfiguration) -> Result<()> {
        let required = configuration
            .limits
            .minimum_free_storage_bytes
            .saturating_add(configuration.limits.submission_bytes as u64);

        if self.attachments.available_space()? < required {
            return Err(AppError::Unavailable);
        }

        Ok(())
    }
}

pub struct IssuedChallenge {
    pub token: String,
    pub expires_at_unix: i64,
    pub expected_work: u64,
}
