use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::{
    error::{AppError, Result},
    infrastructure::{SecretCipher, database::AdminDatabase, github::GithubClient},
};

const RETRY_DELAY: Duration = Duration::from_secs(30);
/// How long a person is left alone after a check of theirs failed, so a token that no longer works does not cost GitHub
/// requests every sweep.
const FAILED_CHECK_PAUSE: Duration = Duration::from_secs(300);

/// Keeps every session's GitHub state current for as long as the process runs.
///
/// Requests do not talk to GitHub to accept a session; they only look at what this stores with it. So it rotates access
/// tokens before they expire, and verifies team membership once a person's last check is half the membership recheck
/// old. Membership is the person's, so one check, made with their most recently used session, covers all of their
/// sessions. A check that fails changes nothing: the session stops being accepted when its last successful check
/// reaches the membership recheck.
pub async fn run_session_verification(
    database: AdminDatabase,
    github: GithubClient,
    cipher: SecretCipher,
) {
    let mut failed_at = HashMap::new();
    loop {
        let delay = match sweep(&database, &github, &cipher, &mut failed_at).await {
            Ok(delay) => delay,
            Err(error) => {
                tracing::warn!(event = "session.verification_sweep_failed", ?error);
                RETRY_DELAY
            }
        };
        tokio::time::sleep(delay).await;
    }
}

async fn sweep(
    database: &AdminDatabase,
    github: &GithubClient,
    cipher: &SecretCipher,
    failed_at: &mut HashMap<i64, Instant>,
) -> Result<Duration> {
    let configuration = database.configuration().await?;
    let recheck = chrono::Duration::seconds(configuration.membership_recheck_seconds as i64);
    let refresh_before =
        chrono::Duration::seconds(configuration.token_refresh_before_seconds as i64);

    // Tokens first, so the checks below use fresh ones.
    for token_hash in database
        .sessions_with_expiring_tokens(refresh_before)
        .await?
    {
        match database
            .refresh_session_token(&token_hash, refresh_before, github, cipher)
            .await
        {
            Ok(_) => {}
            Err(AppError::AuthenticationRequired) => database.revoke_session(&token_hash).await?,
            Err(error) => tracing::warn!(event = "session.background_refresh_failed", ?error),
        }
    }

    failed_at.retain(|_, failed| failed.elapsed() < FAILED_CHECK_PAUSE);
    for session in database.due_session_per_user(recheck / 2).await? {
        if failed_at.contains_key(&session.github_id) {
            continue;
        }

        let verified = async {
            let access_token = cipher.decrypt(&session.encrypted_access_token)?;
            github
                .verify_team_member_identity(
                    &access_token,
                    &configuration.github_authorization_team,
                    session.github_id,
                )
                .await?;
            database
                .refresh_membership_verification_for_user(session.github_id)
                .await
        }
        .await;

        if let Err(error) = verified {
            tracing::warn!(event = "session.background_verification_failed", ?error);
            failed_at.insert(session.github_id, Instant::now());
        }
    }

    // Often enough that a token is rotated, and a person checked, in time.
    let seconds = (configuration.membership_recheck_seconds / 4)
        .min(configuration.token_refresh_before_seconds / 2)
        .clamp(15, 60);
    Ok(Duration::from_secs(seconds))
}
