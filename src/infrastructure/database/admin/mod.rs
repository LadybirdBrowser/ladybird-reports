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

pub(crate) use issues::validate_issue_text;
pub use models::*;

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
