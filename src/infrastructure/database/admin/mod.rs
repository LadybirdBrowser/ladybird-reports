mod authentication;
mod github_sync;
mod issues;
mod maintenance;
mod models;
mod notifications;
mod reports;
mod settings;

pub(crate) use issues::validate_issue_text;
pub use models::*;

use sqlx::PgPool;

use crate::error::Result;

use super::connect_pool;

#[derive(Clone)]
pub struct AdminDatabase {
    pub(super) pool: PgPool,
}

impl AdminDatabase {
    pub async fn connect(database_url: &str) -> Result<Self> {
        Ok(Self {
            pool: connect_pool(database_url, 16).await?,
        })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn healthcheck(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }
}
