use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use chrono::{Duration, Utc};
use cookie::Cookie;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::{error::Result, infrastructure::hash_secret};

use super::{AdminState, templates::redirect_to_login};

#[derive(Clone, Debug)]
pub struct Session {
    pub github_id: i64,
    pub login: String,
    pub csrf_token: String,
    pub token_hash: String,
    pub encrypted_access_token: String,
}

impl Session {
    pub fn verify_csrf(&self, submitted: &str) -> Result<()> {
        let submitted_digest = Sha256::digest(submitted.as_bytes());
        let expected_digest = Sha256::digest(self.csrf_token.as_bytes());

        if !bool::from(submitted_digest.ct_eq(&expected_digest)) {
            return Err(crate::error::AppError::PermissionDenied(
                "Invalid form token",
            ));
        }

        Ok(())
    }

    pub fn github_access_token(&self, state: &AdminState) -> Result<String> {
        state.secret_cipher.decrypt(&self.encrypted_access_token)
    }
}

pub async fn require_session(
    State(state): State<AdminState>,
    mut request: Request,
    next: Next,
) -> Result<Response> {
    let Some(session_token) = request_cookie(&request, "session") else {
        return Ok(redirect_to_login());
    };

    if session_token.len() > 128 {
        return Ok(redirect_to_login());
    }

    let token_hash = hash_secret(session_token);
    let Some(record) = state.database.find_session(&token_hash).await? else {
        return Ok(redirect_to_login());
    };

    let configuration = state.database.configuration().await?;
    let verification_age = Utc::now() - record.membership_verified_at;

    if verification_age >= Duration::seconds(configuration.membership_recheck_seconds as i64) {
        let access_token = state
            .secret_cipher
            .decrypt(&record.encrypted_access_token)?;

        state
            .github
            .verify_maintainer_identity(&access_token, record.github_id)
            .await?;

        state
            .database
            .refresh_membership_verification(&record.token_hash)
            .await?;
    }

    request.extensions_mut().insert(Session {
        github_id: record.github_id,
        login: record.login,
        csrf_token: record.csrf_token,
        token_hash: record.token_hash,
        encrypted_access_token: record.encrypted_access_token,
    });

    Ok(next.run(request).await)
}

pub fn request_cookie(request: &Request, name: &str) -> Option<String> {
    cookie_from_headers(request.headers(), name)
}

pub fn cookie_from_headers(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(Cookie::split_parse)
        .filter_map(std::result::Result::ok)
        .find(|cookie| cookie.name() == name)
        .map(|cookie| cookie.value().to_owned())
}
