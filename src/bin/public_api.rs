use std::sync::Arc;

use ladybird_reports::{
    application::ReportIngestionService,
    error::Result,
    infrastructure::{attachments::FileAttachmentStore, database::IngestDatabase, read_secret},
    runtime::{initialize_tracing, listen_address, required_environment, shutdown_signal},
    web::public::{PublicState, router},
};

#[tokio::main]
async fn main() {
    initialize_tracing();

    if let Err(error) = run().await {
        tracing::error!(event = "startup.failed", ?error, service = "public_api");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let database =
        IngestDatabase::connect(&required_environment("REPORTING_DATABASE_URL")?).await?;
    let attachments = FileAttachmentStore::open(
        std::env::var("ATTACHMENT_ROOT").unwrap_or_else(|_| "./data/attachments".into()),
    )
    .await?;
    let ingestion =
        ReportIngestionService::new(database.clone(), attachments, read_secret("POW_HMAC_KEY")?);

    ingestion.recover_pending_storage().await?;

    let state = PublicState {
        ingestion,
        database,
        client_address_key: Arc::new(read_secret("CLIENT_ADDRESS_HMAC_KEY")?),
    };
    let address = listen_address("PUBLIC_LISTEN_ADDRESS", "0.0.0.0:3001")?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(event = "startup.ready", service = "public_api", %address);

    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    tracing::info!(event = "shutdown.complete", service = "public_api");
    Ok(())
}
