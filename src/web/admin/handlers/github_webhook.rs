use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::{
    error::{AppError, Result},
    infrastructure::github::GithubIssue,
};

use super::super::AdminState;

#[derive(Deserialize)]
struct Repository {
    full_name: String,
}

#[derive(Deserialize)]
struct IssueEvent {
    action: String,
    issue: GithubIssue,
    repository: Repository,
}

pub async fn receive(
    State(state): State<AdminState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode> {
    let secret = state
        .github_webhook_secret
        .as_deref()
        .ok_or(AppError::Unavailable)?;
    verify_signature(secret.as_bytes(), &headers, &body)?;

    if headers
        .get("x-github-event")
        .and_then(|value| value.to_str().ok())
        != Some("issues")
    {
        return Ok(StatusCode::NO_CONTENT);
    }

    let event: IssueEvent = serde_json::from_slice(&body)
        .map_err(|_| AppError::InvalidRequest("Invalid GitHub issue event"))?;
    let configuration = state.database.configuration().await?;
    if event.action != "transferred"
        && !event
            .repository
            .full_name
            .eq_ignore_ascii_case(&configuration.github_repository)
    {
        return Ok(StatusCode::NO_CONTENT);
    }

    let linked_issue = match event.action.as_str() {
        "deleted" => {
            state
                .database
                .mark_github_issue_unavailable(
                    &event.repository.full_name,
                    event.issue.number,
                    Some(event.issue.id),
                    "missing",
                    "webhook",
                )
                .await?
        }
        "transferred" => {
            state
                .database
                .mark_github_issue_unavailable(
                    &event.repository.full_name,
                    event.issue.number,
                    Some(event.issue.id),
                    "moved",
                    "webhook",
                )
                .await?
        }
        "opened" | "edited" | "closed" | "reopened" => {
            state
                .database
                .sync_github_issue(&event.repository.full_name, &event.issue, None, "webhook")
                .await?
        }
        _ => None,
    };

    if let Some(issue_id) = linked_issue {
        tracing::info!(
            event = "github.issue_synchronized",
            %issue_id,
            github_action = event.action,
        );
    }

    Ok(StatusCode::NO_CONTENT)
}

fn verify_signature(secret: &[u8], headers: &HeaderMap, body: &[u8]) -> Result<()> {
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("sha256="))
        .and_then(|value| hex::decode(value).ok())
        .ok_or(AppError::PermissionDenied(
            "Invalid GitHub webhook signature",
        ))?;

    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts keys of any length");
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    if !bool::from(signature.as_slice().ct_eq(expected.as_slice())) {
        return Err(AppError::PermissionDenied(
            "Invalid GitHub webhook signature",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    use super::verify_signature;

    #[test]
    fn rejects_unsigned_and_modified_webhooks() {
        let secret = b"test-webhook-secret";
        let body = br#"{"action":"closed"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        let mut headers = HeaderMap::new();

        assert!(verify_signature(secret, &headers, body).is_err());
        headers.insert(
            "x-hub-signature-256",
            HeaderValue::from_str(&signature).unwrap(),
        );
        assert!(verify_signature(secret, &headers, body).is_ok());
        assert!(verify_signature(secret, &headers, b"modified").is_err());
    }
}
