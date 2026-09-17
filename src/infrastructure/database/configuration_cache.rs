use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde_json::Value;
use sqlx::{PgPool, postgres::PgListener};
use tokio::task::JoinHandle;

use crate::{
    domain::RuntimeConfiguration,
    error::{AppError, Result},
};

const CHANNEL: &str = "ladybird_reports_configuration";
const RECHECK_INTERVAL: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
pub struct ConfigurationCache {
    current: Arc<RwLock<Option<RuntimeConfiguration>>>,
}

impl ConfigurationCache {
    pub fn get(&self) -> Option<RuntimeConfiguration> {
        self.current
            .read()
            .expect("configuration cache lock is not poisoned")
            .clone()
    }

    pub fn set(&self, configuration: RuntimeConfiguration) {
        *self
            .current
            .write()
            .expect("configuration cache lock is not poisoned") = Some(configuration);
    }

    pub async fn start(&self, pool: &PgPool, query: &'static str) -> Result<JoinHandle<()>> {
        // Subscribe before loading the initial snapshot. A change committed
        // during that read will still have a queued notification to process.
        let mut listener = PgListener::connect_with(pool).await?;
        listener.listen(CHANNEL).await?;
        self.reload(pool, query).await?;

        let cache = self.clone();
        let pool = pool.clone();
        Ok(tokio::spawn(async move {
            loop {
                // Notifications are transient. Reload after reconnect and on
                // a timer so a missed event cannot leave a stale cache forever.
                let notification =
                    tokio::time::timeout(RECHECK_INTERVAL, listener.try_recv()).await;
                match notification {
                    Ok(Ok(Some(_))) | Ok(Ok(None)) | Err(_) => {
                        if let Err(error) = cache.reload(&pool, query).await {
                            tracing::warn!(event = "configuration.reload_failed", ?error);
                            tokio::time::sleep(RETRY_DELAY).await;
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(event = "configuration.listener_failed", ?error);
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                }
            }
        }))
    }

    async fn reload(&self, pool: &PgPool, query: &str) -> Result<()> {
        let value: Value = sqlx::query_scalar(query).fetch_one(pool).await?;
        let configuration: RuntimeConfiguration =
            serde_json::from_value(value).map_err(|error| AppError::Internal(error.into()))?;
        configuration.validate()?;
        self.set(configuration);
        Ok(())
    }
}
