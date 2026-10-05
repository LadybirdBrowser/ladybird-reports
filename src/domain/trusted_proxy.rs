use std::{fmt, net::IpAddr, str::FromStr};

use ipnet::IpNet;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::AppError;

const MAXIMUM_HOST_NAME_BYTES: usize = 253;
const MAXIMUM_LABEL_BYTES: usize = 63;

/// A proxy that may identify the original client address through forwarding
/// headers: either a network, or a DNS name for the proxies behind it.
///
/// The configuration keeps the name so that it survives address changes. The
/// public API resolves names once and only ever matches peers against the
/// resulting addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustedProxy {
    Network(IpNet),
    Host(String),
}

impl FromStr for TrustedProxy {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Ok(network) = value.parse::<IpNet>() {
            return Ok(Self::Network(network));
        }

        if value.parse::<IpAddr>().is_ok() {
            return Err(AppError::InvalidRequest(
                "Trusted proxy addresses must use CIDR notation",
            ));
        }

        if !is_host_name(value) {
            return Err(AppError::InvalidRequest(
                "Trusted proxies must be CIDR ranges or DNS names",
            ));
        }

        Ok(Self::Host(value.to_ascii_lowercase()))
    }
}

impl fmt::Display for TrustedProxy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(network) => network.fmt(formatter),
            Self::Host(host) => host.fmt(formatter),
        }
    }
}

impl Serialize for TrustedProxy {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TrustedProxy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

fn is_host_name(value: &str) -> bool {
    if value.is_empty() || value.len() > MAXIMUM_HOST_NAME_BYTES {
        return false;
    }

    value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= MAXIMUM_LABEL_BYTES
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_networks_and_host_names() {
        assert_eq!(
            "172.16.1.0/24".parse::<TrustedProxy>().unwrap(),
            TrustedProxy::Network("172.16.1.0/24".parse().unwrap())
        );
        assert_eq!(
            "fd44:d044:fd92::/64".parse::<TrustedProxy>().unwrap(),
            TrustedProxy::Network("fd44:d044:fd92::/64".parse().unwrap())
        );
        assert_eq!(
            "Coolify-Proxy".parse::<TrustedProxy>().unwrap(),
            TrustedProxy::Host("coolify-proxy".into())
        );
        assert_eq!(
            "gateway.vpn.ladybird.org".parse::<TrustedProxy>().unwrap(),
            TrustedProxy::Host("gateway.vpn.ladybird.org".into())
        );
    }

    #[test]
    fn rejects_bare_addresses_and_malformed_names() {
        for value in [
            "10.0.0.1", "::1", "", "-proxy", "proxy-", "a..b", ".proxy", "proxy.", "pro xy",
            "proxy/24", "proxy:80",
        ] {
            assert!(value.parse::<TrustedProxy>().is_err(), "{value:?}");
        }

        assert!("a".repeat(64).parse::<TrustedProxy>().is_err());
        assert!(["a"; 130].join(".").parse::<TrustedProxy>().is_err());
    }

    #[test]
    fn serializes_as_the_configured_string() {
        let stored = serde_json::json!(["10.0.0.0/8", "coolify-proxy"]);
        let proxies: Vec<TrustedProxy> = serde_json::from_value(stored.clone()).unwrap();

        assert_eq!(serde_json::to_value(&proxies).unwrap(), stored);
        assert!(
            serde_json::from_value::<Vec<TrustedProxy>>(serde_json::json!(["10.0.0.1"])).is_err()
        );
    }
}
