use std::collections::{HashMap, HashSet};

use axum::{
    Extension, Json,
    extract::{Multipart, State},
    http::HeaderMap,
};
use serde::{Deserialize, Serialize};

use crate::{
    application::PrepareSubmissionOutcome,
    domain::AttachmentId,
    error::{AppError, Result},
};

use super::{PublicState, rate_limit::ClientAddressKey};

const MANIFEST_PART_NAME: &str = "manifest";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChallengeRequest {
    manifest_digest: String,
}

#[derive(Serialize)]
pub struct ChallengeResponse {
    algorithm: &'static str,
    token: String,
    expected_work: u64,
    expires_at_unix: i64,
}

#[derive(Serialize)]
pub struct ReportResponse {
    receipt: String,
}

pub async fn issue_challenge(
    State(state): State<PublicState>,
    Json(request): Json<ChallengeRequest>,
) -> Result<Json<ChallengeResponse>> {
    let challenge = state
        .ingestion
        .issue_challenge(&request.manifest_digest)
        .await?;

    Ok(Json(ChallengeResponse {
        algorithm: "sha256-seed-nonce-le-v1",
        token: challenge.token,
        expected_work: challenge.expected_work,
        expires_at_unix: challenge.expires_at_unix,
    }))
}

pub async fn submit_report(
    State(state): State<PublicState>,
    Extension(client): Extension<ClientAddressKey>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ReportResponse>> {
    let challenge_token = required_header(&headers, "x-ladybird-challenge")?;
    let nonce = required_header(&headers, "x-ladybird-nonce")?
        .parse()
        .map_err(|_| AppError::InvalidRequest("Invalid proof-of-work nonce"))?;

    let configuration = state.ingestion.configuration().await?;
    let manifest_bytes = read_manifest(&mut multipart, configuration.limits.metadata_bytes).await?;

    let prepared = state
        .ingestion
        .prepare_submission(challenge_token, nonce, &manifest_bytes)
        .await?;

    let prepared = match prepared {
        PrepareSubmissionOutcome::Ready(prepared) => *prepared,
        PrepareSubmissionOutcome::Existing(report_id) => {
            return Ok(Json(ReportResponse {
                receipt: report_id.to_string(),
            }));
        }
    };

    let upload_id = prepared.upload_id;
    let lease_acquired = match state
        .ingestion
        .acquire_upload_lease(upload_id, &client.0, &configuration)
        .await
    {
        Ok(acquired) => acquired,
        Err(error) => {
            state.ingestion.abandon_submission(upload_id).await;
            return Err(error);
        }
    };

    if !lease_acquired {
        state.ingestion.abandon_submission(upload_id).await;
        return Err(AppError::RateLimited);
    }

    let deadline = std::time::Duration::from_secs(configuration.limits.upload_timeout_seconds);
    let result = tokio::time::timeout(
        deadline,
        receive_and_accept(&state, prepared, multipart, manifest_bytes.len(), &client.0),
    )
    .await
    .map_err(|_| AppError::DeadlineExceeded)
    .and_then(|result| result);

    state.ingestion.release_upload_lease(upload_id).await;

    if result.is_err() {
        state.ingestion.abandon_submission(upload_id).await;
    }

    result.map(|report_id| {
        Json(ReportResponse {
            receipt: report_id.to_string(),
        })
    })
}

async fn receive_and_accept(
    state: &PublicState,
    prepared: crate::application::PreparedSubmission,
    mut multipart: Multipart,
    manifest_size: usize,
    source_client_key: &str,
) -> Result<crate::domain::ReportId> {
    let configuration = state.ingestion.configuration().await?;
    let attachments = prepared
        .manifest
        .attachments
        .iter()
        .map(|attachment| (attachment.id, attachment))
        .collect::<HashMap<_, _>>();

    let mut received = HashSet::new();
    let mut total_bytes = manifest_size as u64;

    while let Some(mut part) = multipart
        .next_field()
        .await
        .map_err(|_| AppError::InvalidRequest("Invalid multipart body"))?
    {
        let client_id = part
            .name()
            .and_then(|name| name.parse::<AttachmentId>().ok())
            .ok_or(AppError::InvalidRequest("Invalid attachment part name"))?;

        let manifest = attachments
            .get(&client_id)
            .ok_or(AppError::InvalidRequest("Unexpected attachment"))?;

        if !received.insert(client_id) {
            return Err(AppError::InvalidRequest("Duplicate attachment"));
        }

        let mut writer = prepared
            .staging
            .create_writer(client_id, manifest.size)
            .await?;

        while let Some(chunk) = part
            .chunk()
            .await
            .map_err(|_| AppError::InvalidRequest("Invalid attachment stream"))?
        {
            total_bytes = total_bytes
                .checked_add(chunk.len() as u64)
                .ok_or(AppError::PayloadTooLarge)?;

            if total_bytes > configuration.limits.submission_bytes as u64 {
                return Err(AppError::PayloadTooLarge);
            }

            writer.write(&chunk).await?;
        }

        writer.finish(manifest.size, &manifest.sha256).await?;
    }

    if received.len() != attachments.len() {
        return Err(AppError::InvalidRequest("Missing attachment"));
    }

    state
        .ingestion
        .accept_submission(prepared, source_client_key)
        .await
}

async fn read_manifest(multipart: &mut Multipart, maximum_bytes: usize) -> Result<Vec<u8>> {
    let mut part = multipart
        .next_field()
        .await
        .map_err(|_| AppError::InvalidRequest("Invalid multipart body"))?
        .ok_or(AppError::InvalidRequest("Missing report manifest"))?;

    if part.name() != Some(MANIFEST_PART_NAME) {
        return Err(AppError::InvalidRequest(
            "Report manifest must be the first part",
        ));
    }

    let mut manifest = Vec::new();

    while let Some(chunk) = part
        .chunk()
        .await
        .map_err(|_| AppError::InvalidRequest("Invalid report manifest"))?
    {
        if manifest.len() + chunk.len() > maximum_bytes {
            return Err(AppError::PayloadTooLarge);
        }

        manifest.extend_from_slice(&chunk);
    }

    Ok(manifest)
}

fn required_header<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::InvalidRequest(
            "Missing or invalid submission header",
        ))
}

pub async fn live() -> &'static str {
    "ok"
}

pub async fn ready(State(state): State<PublicState>) -> Result<&'static str> {
    state.ingestion.healthcheck().await?;
    Ok("ok")
}
