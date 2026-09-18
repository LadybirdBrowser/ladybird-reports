use axum::{
    extract::{Request, State},
    http::{HeaderMap, header::SET_COOKIE},
    middleware::Next,
    response::Response,
};
use chrono::{Duration, Utc};
use cookie::Cookie;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::{
    error::{AppError, Result},
    infrastructure::hash_secret,
};

use super::{AdminState, authentication::session_cookie, templates::redirect_to_login};

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
        return Ok(redirect_to_login(request.method(), request.uri()));
    };

    if session_token.len() > 128 {
        return Ok(redirect_to_login(request.method(), request.uri()));
    }

    let token_hash = hash_secret(&session_token);
    let Some(mut record) = state.database.find_session(&token_hash).await? else {
        return Ok(redirect_to_login(request.method(), request.uri()));
    };

    let configuration = state.database.configuration().await?;
    let refresh_before = Duration::seconds(configuration.token_refresh_before_seconds as i64);

    if record
        .access_token_expires_at
        .is_some_and(|expiry| expiry <= Utc::now() + refresh_before)
    {
        match state
            .database
            .refresh_session_token(
                &token_hash,
                refresh_before,
                &state.github,
                &state.secret_cipher,
            )
            .await
        {
            Ok(Some(encrypted_access_token)) => {
                record.encrypted_access_token = encrypted_access_token
            }
            Ok(None) => return Ok(redirect_to_login(request.method(), request.uri())),
            Err(AppError::AuthenticationRequired) => {
                state.database.revoke_session(&token_hash).await?;
                return Ok(redirect_to_login(request.method(), request.uri()));
            }
            Err(error) => return Err(error),
        }
    }

    let verification_age = Utc::now() - record.membership_verified_at;

    if verification_age >= Duration::seconds(configuration.membership_recheck_seconds as i64) {
        let access_token = state
            .secret_cipher
            .decrypt(&record.encrypted_access_token)?;

        state
            .github
            .verify_team_member_identity(
                &access_token,
                &configuration.github_authorization_team,
                record.github_id,
            )
            .await?;

        state
            .database
            .refresh_membership_verification(&record.token_hash)
            .await?;
    }

    let extended = state
        .database
        .extend_session(
            &token_hash,
            Duration::seconds(configuration.session_lifetime_seconds as i64),
        )
        .await?;

    if record.encrypted_refresh_token.is_some() && !extended {
        return Ok(redirect_to_login(request.method(), request.uri()));
    }

    request.extensions_mut().insert(Session {
        github_id: record.github_id,
        login: record.login,
        csrf_token: record.csrf_token,
        token_hash: record.token_hash,
        encrypted_access_token: record.encrypted_access_token,
    });

    let mut response = next.run(request).await;

    if extended
        && !response.headers().get_all(SET_COOKIE).iter().any(|cookie| {
            cookie
                .to_str()
                .is_ok_and(|cookie| cookie.starts_with("session="))
        })
    {
        let secure = configuration.admin_base_url.starts_with("https:");
        response.headers_mut().append(
            SET_COOKIE,
            session_cookie(
                "session",
                &session_token,
                configuration.session_lifetime_seconds as i64,
                secure,
            )
            .parse()
            .expect("generated cookie is valid"),
        );
    }

    Ok(response)
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
