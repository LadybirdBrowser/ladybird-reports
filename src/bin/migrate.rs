use ladybird_reports::{
    error::{AppError, Result},
    runtime::{initialize_tracing, required_environment},
};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() {
    initialize_tracing();

    if let Err(error) = run().await {
        tracing::error!(event = "migration.failed", ?error, service = "migrate");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&required_environment("ADMIN_DATABASE_URL")?)
        .await?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|error| AppError::Internal(error.into()))?;

    tracing::info!(event = "database.migrations_complete", service = "migrate");
    Ok(())
}
