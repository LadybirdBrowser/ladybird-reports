use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{
    domain::RuntimeConfiguration,
    error::{AppError, Result},
};

use super::PublicState;

#[derive(Clone)]
pub struct ClientAddressKey(pub String);

pub async fn limit_public_request(
    State(state): State<PublicState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Result<Response> {
    if request.uri().path().starts_with("/health/") {
        return Ok(next.run(request).await);
    }

    let configuration = state.database.configuration().await?;
    let client_address = resolve_client_address(peer.ip(), request.headers(), &configuration)?;
    let client_key = keyed_address(&state, client_address);

    if state
        .database
        .source_has_active_rate_limit(&client_key)
        .await?
    {
        return Err(AppError::RateLimited);
    }

    enforce_limits(&state, &configuration, &client_key, request.uri().path()).await?;

    request
        .extensions_mut()
        .insert(ClientAddressKey(client_key));

    Ok(next.run(request).await)
}

async fn enforce_limits(
    state: &PublicState,
    configuration: &RuntimeConfiguration,
    client_key: &str,
    path: &str,
) -> Result<()> {
    let limits = &configuration.limits;

    consume(
        state,
        &format!("public:minute:{client_key}"),
        limits.public_requests_per_minute,
        60,
        limits.public_request_burst,
    )
    .await?;

    let (category, per_minute, burst, per_hour) = if path.ends_with("/challenges") {
        (
            "challenge",
            limits.challenges_per_minute,
            limits.challenge_burst,
            limits.challenges_per_hour,
        )
    } else {
        (
            "submission",
            limits.submissions_per_minute,
            limits.submission_burst,
            limits.submissions_per_hour,
        )
    };

    consume(
        state,
        &format!("{category}:minute:{client_key}"),
        per_minute,
        60,
        burst,
    )
    .await?;

    consume(
        state,
        &format!("{category}:hour:{client_key}"),
        per_hour,
        3600,
        per_hour,
    )
    .await
}

async fn consume(
    state: &PublicState,
    key: &str,
    refill_count: u32,
    refill_seconds: u32,
    capacity: u32,
) -> Result<()> {
    let accepted = state
        .database
        .consume_rate_limit(key, refill_count, refill_seconds, capacity)
        .await?;

    if !accepted {
        return Err(AppError::RateLimited);
    }

    Ok(())
}

fn resolve_client_address(
    peer: IpAddr,
    headers: &HeaderMap,
    configuration: &RuntimeConfiguration,
) -> Result<IpAddr> {
    let trusted = |address: &IpAddr| {
        configuration
            .trusted_proxies
            .iter()
            .any(|network| network.contains(address))
    };

    if !trusted(&peer) {
        return Ok(peer);
    }

    let Some(forwarded) = headers.get("x-forwarded-for") else {
        return Ok(peer);
    };

    let forwarded = forwarded
        .to_str()
        .map_err(|_| AppError::InvalidRequest("Invalid forwarding header"))?;

    if forwarded.len() > 2048 {
        return Err(AppError::InvalidRequest("Invalid forwarding header"));
    }

    let mut current = peer;

    for value in forwarded.split(',').rev() {
        if !trusted(&current) {
            break;
        }

        current = value
            .trim()
            .parse()
            .map_err(|_| AppError::InvalidRequest("Invalid forwarding header"))?;
    }

    Ok(current)
}

fn keyed_address(state: &PublicState, address: IpAddr) -> String {
    let address = normalize_address(address);
    let mut mac = Hmac::<Sha256>::new_from_slice(state.client_address_key.as_ref())
        .expect("HMAC accepts a 32-byte key");

    mac.update(address.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn normalize_address(address: IpAddr) -> String {
    match address {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return mapped.to_string();
            }

            // IPv6 clients are limited by /64. This avoids retaining a second
            // full-address bucket while still resisting cheap address rotation.
            format!("v6-{:016x}", (u128::from(address) >> 64) as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_forwarding_from_untrusted_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "8.8.8.8".parse().unwrap());

        let peer = "127.0.0.1".parse().unwrap();
        let resolved = resolve_client_address(peer, &headers, &RuntimeConfiguration::default())
            .expect("resolve address");

        assert_eq!(resolved, peer);
    }

    #[test]
    fn stops_at_first_untrusted_proxy_hop() {
        let configuration = RuntimeConfiguration {
            trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
            ..RuntimeConfiguration::default()
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "1.1.1.1, 8.8.8.8, 10.0.0.2".parse().unwrap(),
        );

        let resolved =
            resolve_client_address("10.0.0.1".parse().unwrap(), &headers, &configuration)
                .expect("resolve address");

        assert_eq!(resolved, "8.8.8.8".parse::<IpAddr>().unwrap());
    }
}
