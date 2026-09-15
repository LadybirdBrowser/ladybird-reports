use chrono::{DateTime, Duration, Utc};
use sqlx::Row;

use crate::{error::Result, infrastructure::database::AdminDatabase};

use super::SessionRecord;

impl AdminDatabase {
    pub async fn store_oauth_state(&self, state_hash: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO oauth_states (state_hash, expires_at)
             VALUES ($1, now() + interval '10 minutes')",
        )
        .bind(state_hash)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn consume_oauth_state(&self, state_hash: &str) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM oauth_states
             WHERE state_hash = $1 AND expires_at > now()",
        )
        .bind(state_hash)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() == 1)
    }

    pub async fn create_session(
        &self,
        github_id: i64,
        login: &str,
        token_hash: &str,
        encrypted_access_token: &str,
        csrf_token: &str,
        lifetime: Duration,
    ) -> Result<()> {
        let expires_at = Utc::now() + lifetime;
        let mut transaction = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO maintainers (github_id, login)
             VALUES ($1, $2)
             ON CONFLICT (github_id) DO UPDATE SET login = excluded.login",
        )
        .bind(github_id)
        .bind(login)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO sessions (
                token_hash,
                github_id,
                encrypted_access_token,
                csrf_token,
                expires_at
             )
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(token_hash)
        .bind(github_id)
        .bind(encrypted_access_token)
        .bind(csrf_token)
        .bind(expires_at)
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
            membership_verified_at: row.get("membership_verified_at"),
            expires_at: row.get("expires_at"),
        }))
    }

    pub async fn refresh_membership_verification(&self, token_hash: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET membership_verified_at = now() WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn cleanup_authentication_state(&self, before: DateTime<Utc>) -> Result<()> {
        sqlx::query("DELETE FROM oauth_states WHERE expires_at < $1")
            .bind(before)
            .execute(&self.pool)
            .await?;

        sqlx::query("DELETE FROM sessions WHERE expires_at < $1")
            .bind(before)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}
