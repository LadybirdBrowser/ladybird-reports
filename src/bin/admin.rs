use std::sync::Arc;

use ladybird_reports::{
    error::Result,
    infrastructure::{
        SecretCipher,
        attachments::FileAttachmentStore,
        database::{AdminDatabase, initialize_database},
        github::GithubClient,
        read_secret,
    },
    runtime::{initialize_tracing, listen_address, required_environment, shutdown_signal},
    web::admin::{AdminState, router},
};

#[tokio::main]
async fn main() {
    initialize_tracing();

    if let Err(error) = run().await {
        tracing::error!(event = "startup.failed", ?error, service = "admin");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let admin_database_url = required_environment("ADMIN_DATABASE_URL")?;
    let reporting_database_url = std::env::var("REPORTING_DATABASE_URL").ok();
    let secret_cipher = SecretCipher::new(read_secret("SESSION_ENCRYPTION_KEY")?);

    let (pool, bootstrap) = initialize_database(
        &admin_database_url,
        reporting_database_url.as_deref(),
        &secret_cipher,
    )
    .await?;

    if bootstrap.generated_reporting_database_url.is_some() {
        tracing::warn!(
            event = "startup.reporting_setup_required",
            "Reporting database credentials were generated; sign in to the admin UI to retrieve them"
        );
    }

    let attachments = FileAttachmentStore::open(
        std::env::var("ATTACHMENT_ROOT").unwrap_or_else(|_| "./data/attachments".into()),
    )
    .await?;
    let github = GithubClient::new(
        required_environment("GITHUB_CLIENT_ID")?,
        required_environment("GITHUB_CLIENT_SECRET")?,
    )?;
    let state = AdminState {
        database: AdminDatabase::from_pool(pool),
        attachments,
        github,
        secret_cipher,
        bootstrap_reporting_database_url: bootstrap.generated_reporting_database_url.map(Arc::from),
    };

    let address = listen_address("ADMIN_LISTEN_ADDRESS", "0.0.0.0:3000")?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(event = "startup.ready", service = "admin", %address);

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!(event = "shutdown.complete", service = "admin");
    Ok(())
}
