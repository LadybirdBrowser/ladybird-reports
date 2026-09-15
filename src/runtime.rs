use std::net::SocketAddr;

use crate::error::{AppError, Result};

pub const APPLICATION_VERSION: &str = match option_env!("LADYBIRD_REPORTS_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

pub fn initialize_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        "ladybird_reports=info,admin=info,public_api=info,migrate=info,tower_http=info".into()
    });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false)
        .compact()
        .init();
}

pub fn required_environment(name: &str) -> Result<String> {
    std::env::var(name).map_err(|_| AppError::Internal(anyhow::anyhow!("{name} is required")))
}

pub fn listen_address(name: &str, default: &str) -> Result<SocketAddr> {
    std::env::var(name)
        .unwrap_or_else(|_| default.to_owned())
        .parse()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("{name} is not a socket address")))
}

pub async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install termination handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
}
