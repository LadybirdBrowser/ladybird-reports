use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use serde_json::Value;
use sqlx::{PgPool, postgres::PgListener};
use tokio::task::JoinHandle;

use crate::{
    domain::RuntimeConfiguration,
    error::{AppError, Result},
};

use super::proxy_addresses::ProxyAddresses;

const CHANNEL: &str = "ladybird_reports_configuration";
const RECHECK_INTERVAL: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(5);
const PROXY_ADDRESS_LIFETIME: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Default)]
pub struct ConfigurationCache {
    current: Arc<RwLock<Option<Arc<RuntimeConfiguration>>>>,
    proxy_addresses: Option<Arc<ProxyAddresses>>,
}

impl ConfigurationCache {
    /// A cache that resolves the DNS names among the trusted proxies and uses
    /// the addresses for at most five minutes.
    pub fn resolving_proxy_hosts() -> Self {
        Self {
            proxy_addresses: Some(Arc::new(ProxyAddresses::new(
                PROXY_ADDRESS_LIFETIME,
                RECHECK_INTERVAL,
            ))),
            ..Self::default()
        }
    }

    /// Returns a shared snapshot; reading it never copies the configuration.
    pub fn get(&self) -> Option<Arc<RuntimeConfiguration>> {
        self.current
            .read()
            .expect("configuration cache lock is not poisoned")
            .clone()
    }

    pub fn set(&self, configuration: RuntimeConfiguration) {
        *self
            .current
            .write()
            .expect("configuration cache lock is not poisoned") = Some(Arc::new(configuration));
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

    async fn reload(&self, pool: &PgPool, query: &'static str) -> Result<()> {
        let value: Value = sqlx::query_scalar(query).fetch_one(pool).await?;
        let mut configuration: RuntimeConfiguration =
            serde_json::from_value(value).map_err(AppError::internal)?;
        configuration.validate()?;
        match &self.proxy_addresses {
            Some(proxy_addresses) => proxy_addresses.apply(&mut configuration).await?,
            None => configuration.resolve_trusted_networks(&HashMap::new()),
        }
        self.set(configuration);
        Ok(())
    }
}
