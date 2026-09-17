use std::sync::Arc;

use ladybird_reports::{
    application::DiscordNotificationService,
    error::Result,
    infrastructure::{
        SecretCipher,
        attachments::FileAttachmentStore,
        database::{AdminDatabase, initialize_database},
        discord::DiscordClient,
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
    let github_webhook_secret = std::env::var("GITHUB_WEBHOOK_SECRET")
        .ok()
        .map(|secret| {
            if secret.len() < 32 {
                return Err(ladybird_reports::error::AppError::InvalidRequest(
                    "GITHUB_WEBHOOK_SECRET must be at least 32 characters",
                ));
            }
            Ok(Arc::<str>::from(secret))
        })
        .transpose()?;
    let database = AdminDatabase::from_pool(pool);
    let configuration_listener = database.start_configuration_cache().await?;
    let discord_notifications =
        DiscordNotificationService::new(database.clone(), DiscordClient::new()?);
    let state = AdminState {
        database,
        attachments,
        github,
        github_webhook_secret,
        secret_cipher,
        bootstrap_reporting_database_url: bootstrap.generated_reporting_database_url.map(Arc::from),
    };
    let maintenance = tokio::spawn(run_maintenance(state.clone()));
    let discord_notifications = tokio::spawn(discord_notifications.run());
    let stack_indexer = tokio::spawn(run_stack_indexer(state.database.clone()));

    let address = listen_address("ADMIN_LISTEN_ADDRESS", "0.0.0.0:3000")?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(event = "startup.ready", service = "admin", %address);

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    maintenance.abort();
    let _ = maintenance.await;
    discord_notifications.abort();
    let _ = discord_notifications.await;
    stack_indexer.abort();
    let _ = stack_indexer.await;
    configuration_listener.abort();
    let _ = configuration_listener.await;

    tracing::info!(event = "shutdown.complete", service = "admin");
    Ok(())
}

async fn run_stack_indexer(database: AdminDatabase) {
    loop {
        let delay = match database.index_pending_stack_traces().await {
            Ok(50) => std::time::Duration::from_millis(100),
            Ok(0) => std::time::Duration::from_secs(1),
            Ok(count) => {
                tracing::info!(event = "stack_index.batch_complete", count);
                std::time::Duration::from_secs(1)
            }
            Err(error) => {
                tracing::warn!(event = "stack_index.failed", ?error);
                std::time::Duration::from_secs(30)
            }
        };
        tokio::time::sleep(delay).await;
    }
}

async fn run_maintenance(state: AdminState) {
    loop {
        let interval_seconds = match state.database.configuration().await {
            Ok(configuration) => {
                let result = state
                    .database
                    .begin_maintenance_sweep(
                        configuration.maintenance.report_retention_days,
                        configuration.maintenance.submission_source_retention_days,
                    )
                    .await;

                match result {
                    Ok(result) => {
                        let mut reports_deleted = 0_u64;
                        for report_id in result.reports_ready_for_purge {
                            if let Err(error) = state.attachments.remove_report(report_id).await {
                                tracing::warn!(
                                    event = "maintenance.report_files_failed",
                                    ?error,
                                    %report_id,
                                );
                                continue;
                            }

                            match state.database.finish_report_purge(report_id).await {
                                Ok(true) => reports_deleted += 1,
                                Ok(false) => {}
                                Err(error) => tracing::warn!(
                                    event = "maintenance.report_purge_failed",
                                    ?error,
                                    %report_id,
                                ),
                            }
                        }

                        if result.sessions_deleted > 0
                            || result.oauth_states_deleted > 0
                            || result.submission_sources_deleted > 0
                            || result.inactive_source_blocks_deleted > 0
                            || reports_deleted > 0
                        {
                            tracing::info!(
                                event = "maintenance.admin_sweep_completed",
                                sessions_deleted = result.sessions_deleted,
                                oauth_states_deleted = result.oauth_states_deleted,
                                submission_sources_deleted = result.submission_sources_deleted,
                                inactive_source_blocks_deleted =
                                    result.inactive_source_blocks_deleted,
                                reports_deleted,
                            );
                        }
                    }
                    Err(error) => tracing::warn!(event = "maintenance.admin_sweep_failed", ?error,),
                }

                configuration.maintenance.sweep_interval_seconds
            }
            Err(error) => {
                tracing::warn!(
                    event = "maintenance.configuration_failed",
                    ?error,
                    service = "admin",
                );
                900
            }
        };

        tokio::time::sleep(std::time::Duration::from_secs(interval_seconds)).await;
    }
}
