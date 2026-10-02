mod authentication;
mod failure_reasons;
mod github_duplicates;
mod github_sync;
mod issues;
mod maintenance;
mod models;
mod notifications;
mod reports;
mod settings;
mod stack_signatures;

pub use failure_reasons::FAILURE_REASON_BATCH_SIZE;
pub(crate) use issues::validate_issue_text;
pub use models::*;
pub use reports::{REPORT_PAGE_SIZE, SEARCH_VALUE_LIMIT};
pub use stack_signatures::STACK_INDEX_BATCH_SIZE;

use sqlx::PgPool;

use crate::error::Result;

use super::{ConfigurationCache, connect_pool};

#[derive(Clone)]
pub struct AdminDatabase {
    pub(super) pool: PgPool,
    pub(super) configuration_cache: ConfigurationCache,
}

impl AdminDatabase {
    pub async fn connect(database_url: &str) -> Result<Self> {
        Ok(Self {
            pool: connect_pool(database_url, 16).await?,
            configuration_cache: ConfigurationCache::default(),
        })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            configuration_cache: ConfigurationCache::default(),
        }
    }

    pub async fn start_configuration_cache(&self) -> Result<tokio::task::JoinHandle<()>> {
        self.configuration_cache
            .start(
                &self.pool,
                "SELECT value FROM runtime_configuration WHERE singleton = true",
            )
            .await
    }

    pub async fn healthcheck(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }
}

/// Appends one row to the audit log. Pass `serde_json::json!({})` when the
/// event has no details; that is what the column defaults to.
async fn insert_audit_event<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    actor: Option<i64>,
    action: &str,
    entity_id: Option<uuid::Uuid>,
    details: serde_json::Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_events (actor, action, entity_id, details)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(actor)
    .bind(action)
    .bind(entity_id)
    .bind(details)
    .execute(executor)
    .await?;

    Ok(())
}
