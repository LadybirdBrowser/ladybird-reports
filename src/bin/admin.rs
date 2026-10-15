use std::sync::Arc;

use tokio::task::JoinSet;

use ladybird_reports::{
    application::{
        DiscordNotificationService, backfill_failure_reasons, run_session_verification,
        run_stack_indexer,
    },
    error::{AppError, Result},
    infrastructure::{
        SecretCipher,
        attachments::FileAttachmentStore,
        database::{AdminDatabase, FAILURE_REASON_BATCH_SIZE, initialize_database},
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
    // Background tasks run for as long as the process does. None of them should end, so one that does, by a panic or
    // otherwise, takes the process down for the platform to restart, rather than leaving a service that quietly stops
    // doing part of its work.
    let mut background_tasks = JoinSet::new();
    background_tasks.spawn({
        let state = state.clone();
        async move {
            run_maintenance(state).await;
            "maintenance"
        }
    });
    background_tasks.spawn(async move {
        discord_notifications.run().await;
        "discord_notifications"
    });
    background_tasks.spawn({
        let database = state.database.clone();
        async move {
            run_stack_indexer(database).await;
            "stack_indexer"
        }
    });
    background_tasks.spawn({
        let state = state.clone();
        async move {
            run_failure_reason_backfill(state).await;
            "failure_reason_backfill"
        }
    });
    background_tasks.spawn({
        let state = state.clone();
        async move {
            run_github_duplicates(state).await;
            "github_duplicates"
        }
    });
    background_tasks.spawn({
        let database = state.database.clone();
        let github = state.github.clone();
        let cipher = state.secret_cipher.clone();
        async move {
            run_session_verification(database, github, cipher).await;
            "session_verification"
        }
    });

    let address = listen_address("ADMIN_LISTEN_ADDRESS", "0.0.0.0:3000")?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(event = "startup.ready", service = "admin", %address);

    let stopped_task = tokio::select! {
        result = axum::serve(listener, router(state)).with_graceful_shutdown(shutdown_signal()) => {
            result?;
            None
        }
        ended = background_tasks.join_next() => ended,
    };

    background_tasks.abort_all();
    while background_tasks.join_next().await.is_some() {}
    configuration_listener.abort();
    let _ = configuration_listener.await;

    if let Some(ended) = stopped_task {
        return Err(match ended {
            Ok(task) => {
                tracing::error!(event = "background_task.ended", task);
                AppError::internal(anyhow::anyhow!("background task {task} ended"))
            }
            Err(error) => {
                tracing::error!(event = "background_task.failed", ?error);
                AppError::internal(error)
            }
        });
    }

    tracing::info!(event = "shutdown.complete", service = "admin");
    Ok(())
}

async fn run_github_duplicates(state: AdminState) {
    if !state.github.has_installation_credentials() {
        tracing::warn!(
            event = "github.duplicate_processing_disabled",
            "Configure GITHUB_APP_PRIVATE_KEY_BASE64 to process duplicate closures"
        );
        // Not finishing, as a task that finished would end the process.
        std::future::pending::<()>().await;
        return;
    }

    loop {
        let delay = match state.database.claim_github_duplicate().await {
            Ok(Some(job)) => {
                let result = async {
                    let canonical = state
                        .github
                        .duplicate_of(job.installation_id, &job.repository, job.number)
                        .await?;
                    state
                        .database
                        .finish_github_duplicate(&job, canonical.as_ref())
                        .await
                }
                .await;

                match result {
                    Ok(Some(destination)) => tracing::info!(
                        event = "github.duplicate_processed",
                        source = %job.source_issue_id,
                        %destination,
                    ),
                    Ok(None) => tracing::info!(
                        event = "github.duplicate_no_target",
                        source = %job.source_issue_id,
                    ),
                    Err(error) => {
                        tracing::warn!(
                            event = "github.duplicate_processing_failed",
                            source = %job.source_issue_id,
                            attempt = job.attempt_count,
                            ?error,
                        );
                        if let Err(retry_error) = state.database.retry_github_duplicate(&job).await
                        {
                            tracing::warn!(event = "github.duplicate_retry_failed", ?retry_error);
                        }
                    }
                }
                std::time::Duration::from_millis(100)
            }
            Ok(None) => std::time::Duration::from_secs(5),
            Err(error) => {
                tracing::warn!(event = "github.duplicate_claim_failed", ?error);
                std::time::Duration::from_secs(30)
            }
        };
        tokio::time::sleep(delay).await;
    }
}

async fn run_failure_reason_backfill(state: AdminState) {
    loop {
        let delay = match backfill_failure_reasons(&state.database, &state.attachments).await {
            Ok(FAILURE_REASON_BATCH_SIZE) => std::time::Duration::from_millis(100),
            Ok(0) => std::time::Duration::from_secs(30),
            Ok(count) => {
                tracing::info!(event = "failure_reason.backfill_batch_complete", count);
                std::time::Duration::from_secs(1)
            }
            Err(error) => {
                tracing::warn!(event = "failure_reason.backfill_failed", ?error);
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
                        configuration.maintenance.submission_source_retention_hours,
                        configuration.audit.retention_days,
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
                            || result.audit_events_deleted > 0
                            || reports_deleted > 0
                        {
                            tracing::info!(
                                event = "maintenance.admin_sweep_completed",
                                sessions_deleted = result.sessions_deleted,
                                oauth_states_deleted = result.oauth_states_deleted,
                                submission_sources_deleted = result.submission_sources_deleted,
                                inactive_source_blocks_deleted =
                                    result.inactive_source_blocks_deleted,
                                audit_events_deleted = result.audit_events_deleted,
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
