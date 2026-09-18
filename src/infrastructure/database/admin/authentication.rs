use chrono::Utc;
use sqlx::Row;

use crate::{error::Result, infrastructure::database::AdminDatabase};

use super::{NewSession, SessionRecord};

impl AdminDatabase {
    pub async fn store_oauth_state(&self, state_hash: &str, return_to: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO oauth_states (state_hash, return_to, expires_at)
             VALUES ($1, $2, now() + interval '10 minutes')",
        )
        .bind(state_hash)
        .bind(return_to)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn consume_oauth_state(&self, state_hash: &str) -> Result<Option<String>> {
        sqlx::query_scalar(
            "DELETE FROM oauth_states
             WHERE state_hash = $1 AND expires_at > now()
             RETURNING return_to",
        )
        .bind(state_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn create_session(&self, session: NewSession<'_>) -> Result<()> {
        let expires_at = Utc::now() + session.lifetime;
        let mut transaction = self.pool.begin().await?;

        // Hold a read lock on the policy until the session is committed. A
        // concurrent team change takes the write lock and revokes sessions,
        // so a login verified under the old team cannot slip in afterward.
        let active_team: String = sqlx::query_scalar(
            "SELECT value ->> 'github_authorization_team'
             FROM runtime_configuration
             WHERE singleton = true
             FOR SHARE",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if active_team != session.authorized_team {
            return Err(crate::error::AppError::PermissionDenied("Access denied"));
        }

        sqlx::query(
            "INSERT INTO maintainers (github_id, login)
             VALUES ($1, $2)
             ON CONFLICT (github_id) DO UPDATE SET login = excluded.login",
        )
        .bind(session.github_id)
        .bind(session.login)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO sessions (
                token_hash,
                github_id,
                encrypted_access_token,
                encrypted_refresh_token,
                access_token_expires_at,
                refresh_token_expires_at,
                csrf_token,
                expires_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(session.token_hash)
        .bind(session.github_id)
        .bind(session.encrypted_access_token)
        .bind(session.encrypted_refresh_token)
        .bind(session.access_token_expires_at)
        .bind(session.refresh_token_expires_at)
        .bind(session.csrf_token)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action)
             VALUES ($1, 'session.signed_in')",
        )
        .bind(session.github_id)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn find_session(&self, token_hash: &str) -> Result<Option<SessionRecord>> {
        let row = sqlx::query(
            "SELECT
                sessions.github_id,
                maintainers.login,
                sessions.csrf_token,
                sessions.token_hash,
                sessions.encrypted_access_token,
                sessions.encrypted_refresh_token,
                sessions.access_token_expires_at,
                sessions.refresh_token_expires_at,
                sessions.membership_verified_at,
                sessions.expires_at
             FROM sessions
             JOIN maintainers USING (github_id)
             WHERE sessions.token_hash = $1 AND sessions.expires_at > now()",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| SessionRecord {
            github_id: row.get("github_id"),
            login: row.get("login"),
            csrf_token: row.get("csrf_token"),
            token_hash: row.get("token_hash"),
            encrypted_access_token: row.get("encrypted_access_token"),
            encrypted_refresh_token: row.get("encrypted_refresh_token"),
            access_token_expires_at: row.get("access_token_expires_at"),
            refresh_token_expires_at: row.get("refresh_token_expires_at"),
            membership_verified_at: row.get("membership_verified_at"),
            expires_at: row.get("expires_at"),
        }))
    }

    pub async fn extend_session(
        &self,
        token_hash: &str,
        lifetime: chrono::Duration,
    ) -> Result<bool> {
        let expires_at = Utc::now() + lifetime;
        let result = sqlx::query(
            "UPDATE sessions SET expires_at = $2
             WHERE token_hash = $1 AND expires_at > now() AND encrypted_refresh_token IS NOT NULL",
        )
        .bind(token_hash)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() == 1)
    }

    pub async fn refresh_session_token(
        &self,
        token_hash: &str,
        refresh_before: chrono::Duration,
        github: &crate::infrastructure::github::GithubClient,
        cipher: &crate::infrastructure::SecretCipher,
    ) -> Result<Option<String>> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT encrypted_access_token, encrypted_refresh_token,
                    access_token_expires_at, refresh_token_expires_at
             FROM sessions WHERE token_hash = $1 AND expires_at > now() FOR UPDATE",
        )
        .bind(token_hash)
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let encrypted_access_token: String = row.get("encrypted_access_token");
        let access_expires_at: Option<chrono::DateTime<Utc>> = row.get("access_token_expires_at");
        let Some(access_expires_at) = access_expires_at else {
            return Ok(Some(encrypted_access_token));
        };

        if access_expires_at > Utc::now() + refresh_before {
            return Ok(Some(encrypted_access_token));
        }

        let encrypted_refresh_token: Option<String> = row.get("encrypted_refresh_token");
        let refresh_expires_at: Option<chrono::DateTime<Utc>> = row.get("refresh_token_expires_at");
        let (Some(encrypted_refresh_token), Some(refresh_expires_at)) =
            (encrypted_refresh_token, refresh_expires_at)
        else {
            return Ok(Some(encrypted_access_token));
        };

        if refresh_expires_at <= Utc::now() {
            return Err(crate::error::AppError::AuthenticationRequired);
        }

        let refresh_token = cipher.decrypt(&encrypted_refresh_token)?;
        let tokens = github.refresh_user_token(&refresh_token).await?;
        let (Some(new_refresh_token), Some(access_lifetime), Some(refresh_lifetime)) = (
            tokens.refresh_token,
            tokens.expires_in,
            tokens.refresh_token_expires_in,
        ) else {
            return Err(crate::error::AppError::Unavailable);
        };

        if access_lifetime <= 0 || refresh_lifetime <= 0 {
            return Err(crate::error::AppError::Unavailable);
        }

        let now = Utc::now();
        let encrypted_access_token = cipher.encrypt(&tokens.access_token)?;
        let encrypted_refresh_token = cipher.encrypt(&new_refresh_token)?;

        sqlx::query(
            "UPDATE sessions SET encrypted_access_token = $2,
                    encrypted_refresh_token = $3, access_token_expires_at = $4,
                    refresh_token_expires_at = $5
             WHERE token_hash = $1",
        )
        .bind(token_hash)
        .bind(&encrypted_access_token)
        .bind(encrypted_refresh_token)
        .bind(now + chrono::Duration::seconds(access_lifetime))
        .bind(now + chrono::Duration::seconds(refresh_lifetime))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        tracing::info!(event = "session.github_token_refreshed");
        Ok(Some(encrypted_access_token))
    }

    pub async fn refresh_membership_verification(&self, token_hash: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET membership_verified_at = now() WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn delete_session(&self, token_hash: &str, github_id: i64) -> Result<()> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action)
             VALUES ($1, 'session.signed_out')",
        )
        .bind(github_id)
        .execute(&mut *transaction)
        .await?;

        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;

        Ok(())
    }

    pub async fn revoke_session(&self, token_hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
