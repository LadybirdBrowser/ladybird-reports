use std::{
    collections::HashMap,
    net::IpAddr,
    time::{Duration, Instant},
};

use tokio::sync::Mutex;

use crate::{
    domain::RuntimeConfiguration,
    error::{AppError, Result},
};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

struct Resolution {
    addresses: Vec<IpAddr>,
    resolved_at: Instant,
}

/// Remembers the addresses of the DNS names among the trusted proxies for a
/// limited time, so that resolving never happens while serving a request.
pub struct ProxyAddresses {
    lifetime: Duration,
    refresh_interval: Duration,
    resolutions: Mutex<HashMap<String, Resolution>>,
}

impl ProxyAddresses {
    /// Addresses are used for at most `lifetime`. `refresh_interval` is how
    /// often `apply` runs: a resolution that would be too old by the next run
    /// is refreshed on this one.
    pub fn new(lifetime: Duration, refresh_interval: Duration) -> Self {
        Self {
            lifetime,
            refresh_interval,
            resolutions: Mutex::default(),
        }
    }

    /// Fills `trusted_networks` of the configuration, resolving names that are
    /// new or about to expire. A name that cannot be resolved again keeps its
    /// previous addresses; one that was never resolved is an error.
    pub async fn apply(&self, configuration: &mut RuntimeConfiguration) -> Result<()> {
        let mut resolutions = self.resolutions.lock().await;

        resolutions.retain(|host, _| {
            configuration
                .trusted_proxy_hosts()
                .any(|configured| configured == host)
        });

        for host in configuration.trusted_proxy_hosts() {
            let previous = resolutions.get(host);

            if previous.is_some_and(|previous| !self.expires_before_next_run(previous)) {
                continue;
            }

            match resolve_host(host).await {
                Ok(addresses) => {
                    if previous.map(|previous| &previous.addresses) != Some(&addresses) {
                        tracing::info!(event = "configuration.proxy_resolved", host, ?addresses);
                    }

                    resolutions.insert(
                        host.to_owned(),
                        Resolution {
                            addresses,
                            resolved_at: Instant::now(),
                        },
                    );
                }
                Err(error) if previous.is_some() => {
                    tracing::warn!(event = "configuration.proxy_refresh_failed", host, ?error);
                }
                Err(error) => return Err(error),
            }
        }

        let resolved = resolutions
            .iter()
            .map(|(host, resolution)| (host.clone(), resolution.addresses.clone()))
            .collect();
        configuration.resolve_trusted_networks(&resolved);

        Ok(())
    }

    fn expires_before_next_run(&self, resolution: &Resolution) -> bool {
        resolution.resolved_at.elapsed() + self.refresh_interval >= self.lifetime
    }
}

async fn resolve_host(host: &str) -> Result<Vec<IpAddr>> {
    let failure = |reason: &dyn std::fmt::Display| {
        AppError::Internal(anyhow::anyhow!(
            "cannot resolve trusted proxy {host}: {reason}"
        ))
    };

    let addresses = tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((host, 0)))
        .await
        .map_err(|_| failure(&"timed out"))?
        .map_err(|error| failure(&error))?;

    let mut addresses: Vec<IpAddr> = addresses.map(|address| address.ip()).collect();
    addresses.sort();
    addresses.dedup();

    if addresses.is_empty() {
        return Err(failure(&"no addresses"));
    }

    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use ipnet::IpNet;

    use super::*;

    fn configuration(proxies: &[&str]) -> RuntimeConfiguration {
        RuntimeConfiguration {
            trusted_proxies: proxies.iter().map(|proxy| proxy.parse().unwrap()).collect(),
            ..RuntimeConfiguration::default()
        }
    }

    fn stored(addresses: &ProxyAddresses, host: &str, resolution: Resolution) {
        addresses
            .resolutions
            .try_lock()
            .unwrap()
            .insert(host.to_owned(), resolution);
    }

    #[tokio::test]
    async fn resolves_a_host_name_to_its_addresses() {
        let addresses = resolve_host("localhost").await.expect("resolve localhost");

        assert!(!addresses.is_empty());
        assert!(addresses.iter().all(IpAddr::is_loopback));
    }

    #[tokio::test]
    async fn reports_a_host_name_that_does_not_resolve() {
        let error = resolve_host("does-not-exist.invalid")
            .await
            .expect_err("the .invalid TLD never resolves");

        assert!(error.to_string().contains("does-not-exist.invalid"));
    }

    #[tokio::test]
    async fn trusts_configured_ranges_and_resolved_names() {
        let addresses = ProxyAddresses::new(Duration::from_secs(300), Duration::from_secs(60));
        let mut configuration = configuration(&["10.200.0.2/32", "localhost"]);

        addresses.apply(&mut configuration).await.expect("apply");

        assert!(
            configuration
                .trusted_networks
                .contains(&"10.200.0.2/32".parse::<IpNet>().unwrap())
        );
        assert!(
            configuration
                .trusted_networks
                .iter()
                .any(|network| network.addr().is_loopback())
        );
    }

    #[tokio::test]
    async fn a_name_that_never_resolved_is_an_error() {
        let addresses = ProxyAddresses::new(Duration::from_secs(300), Duration::from_secs(60));
        let mut configuration = configuration(&["does-not-exist.invalid"]);

        assert!(addresses.apply(&mut configuration).await.is_err());
    }

    #[tokio::test]
    async fn fresh_addresses_are_not_looked_up_again() {
        let addresses = ProxyAddresses::new(Duration::from_secs(300), Duration::from_secs(60));
        stored(
            &addresses,
            "does-not-exist.invalid",
            Resolution {
                addresses: vec!["172.16.1.6".parse().unwrap()],
                resolved_at: Instant::now(),
            },
        );
        let mut configuration = configuration(&["does-not-exist.invalid"]);

        addresses.apply(&mut configuration).await.expect("apply");

        // Resolving it again would have failed, so it was served from memory.
        assert_eq!(
            configuration.trusted_networks,
            ["172.16.1.6/32".parse::<IpNet>().unwrap()]
        );
    }

    #[tokio::test]
    async fn expired_addresses_survive_a_failed_refresh() {
        let addresses = ProxyAddresses::new(Duration::ZERO, Duration::ZERO);
        stored(
            &addresses,
            "does-not-exist.invalid",
            Resolution {
                addresses: vec!["172.16.1.6".parse().unwrap()],
                resolved_at: Instant::now(),
            },
        );
        let mut configuration = configuration(&["does-not-exist.invalid"]);

        addresses.apply(&mut configuration).await.expect("apply");

        assert_eq!(
            configuration.trusted_networks,
            ["172.16.1.6/32".parse::<IpNet>().unwrap()]
        );
    }

    #[tokio::test]
    async fn expired_addresses_are_replaced_by_a_new_lookup() {
        let addresses = ProxyAddresses::new(Duration::ZERO, Duration::ZERO);
        stored(
            &addresses,
            "localhost",
            Resolution {
                addresses: vec!["172.16.1.6".parse().unwrap()],
                resolved_at: Instant::now(),
            },
        );
        let mut configuration = configuration(&["localhost"]);

        addresses.apply(&mut configuration).await.expect("apply");

        assert!(
            configuration
                .trusted_networks
                .iter()
                .all(|network| network.addr().is_loopback())
        );
    }

    #[tokio::test]
    async fn names_removed_from_the_configuration_are_forgotten() {
        let addresses = ProxyAddresses::new(Duration::from_secs(300), Duration::from_secs(60));
        stored(
            &addresses,
            "old-proxy",
            Resolution {
                addresses: vec!["172.16.1.9".parse().unwrap()],
                resolved_at: Instant::now(),
            },
        );
        let mut configuration = configuration(&["10.200.0.2/32"]);

        addresses.apply(&mut configuration).await.expect("apply");

        assert!(addresses.resolutions.try_lock().unwrap().is_empty());
        assert_eq!(
            configuration.trusted_networks,
            ["10.200.0.2/32".parse::<IpNet>().unwrap()]
        );
    }
}
