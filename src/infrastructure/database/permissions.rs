use rand::RngCore;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};

use crate::{
    error::{AppError, Result},
    infrastructure::{SecretCipher, random_token},
};

pub struct DatabaseBootstrap {
    pub generated_reporting_database_url: Option<String>,
}

pub async fn initialize_database(
    admin_database_url: &str,
    reporting_database_url: Option<&str>,
    secret_cipher: &SecretCipher,
) -> Result<(PgPool, DatabaseBootstrap)> {
    let admin_pool = PgPoolOptions::new()
        .max_connections(16)
        .connect(admin_database_url)
        .await?;

    sqlx::migrate!("./migrations")
        .run(&admin_pool)
        .await
        .map_err(|error| AppError::Internal(error.into()))?;
    tracing::info!(event = "database.migrations_complete");

    let generated_reporting_database_url = match reporting_database_url {
        Some(database_url) => {
            configure_existing_reporting_role(&admin_pool, database_url).await?;
            clear_pending_bootstrap(&admin_pool).await?;
            tracing::info!(event = "database.reporting_role_verified");
            None
        }
        None => Some(
            load_or_create_reporting_credentials(&admin_pool, admin_database_url, secret_cipher)
                .await?,
        ),
    };

    Ok((
        admin_pool,
        DatabaseBootstrap {
            generated_reporting_database_url,
        },
    ))
}

async fn configure_existing_reporting_role(
    admin_pool: &PgPool,
    reporting_database_url: &str,
) -> Result<()> {
    let reporting_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(reporting_database_url)
        .await?;

    let role: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(&reporting_pool)
        .await?;

    reporting_pool.close().await;
    apply_ingest_permissions(admin_pool, &role).await
}

async fn load_or_create_reporting_credentials(
    admin_pool: &PgPool,
    admin_database_url: &str,
    secret_cipher: &SecretCipher,
) -> Result<String> {
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT encrypted_reporting_database_url
         FROM bootstrap_credentials
         WHERE singleton = true",
    )
    .fetch_optional(admin_pool)
    .await?;

    if let Some(encrypted_url) = existing {
        return secret_cipher.decrypt(&encrypted_url);
    }

    let role = random_reporting_role();
    let password = random_token();

    create_reporting_role(admin_pool, &role, &password).await?;
    apply_ingest_permissions(admin_pool, &role).await?;

    let database_url = reporting_database_url(admin_database_url, &role, &password)?;
    let encrypted_url = secret_cipher.encrypt(&database_url)?;

    sqlx::query(
        "INSERT INTO bootstrap_credentials (encrypted_reporting_database_url)
         VALUES ($1)",
    )
    .bind(encrypted_url)
    .execute(admin_pool)
    .await?;

    tracing::warn!(
        event = "database.reporting_role_created",
        role,
        "Reporting credentials are waiting in the authenticated setup UI"
    );

    Ok(database_url)
}

async fn create_reporting_role(admin_pool: &PgPool, role: &str, password: &str) -> Result<()> {
    // PostgreSQL quotes both the identifier and literal. The resulting statement
    // is executed separately because role names cannot be bind parameters.
    let statement: String = sqlx::query_scalar(
        "SELECT format(
            'CREATE ROLE %I LOGIN PASSWORD %L NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT',
            $1,
            $2
        )",
    )
    .bind(role)
    .bind(password)
    .fetch_one(admin_pool)
    .await?;

    sqlx::query(&statement).execute(admin_pool).await?;
    Ok(())
}

pub async fn apply_ingest_permissions(admin_pool: &PgPool, ingest_role: &str) -> Result<()> {
    verify_ingest_role_exists(admin_pool, ingest_role).await?;

    let quoted_role: String = sqlx::query("SELECT quote_ident($1) AS role")
        .bind(ingest_role)
        .fetch_one(admin_pool)
        .await?
        .get("role");

    let statements = [
        format!("REVOKE ALL ON ALL TABLES IN SCHEMA public FROM {quoted_role}"),
        format!("REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM {quoted_role}"),
        format!("REVOKE ALL ON ALL FUNCTIONS IN SCHEMA public FROM {quoted_role}"),
        format!("GRANT USAGE ON SCHEMA public TO {quoted_role}"),
        format!("GRANT SELECT ON runtime_configuration, field_definitions TO {quoted_role}"),
    ];

    for statement in statements {
        sqlx::query(&statement).execute(admin_pool).await?;
    }

    let functions = [
        "issue_challenge(uuid, text, text, timestamptz)",
        "consume_rate_limit(text, double precision, double precision, double precision)",
        "source_has_active_rate_limit(text)",
        "acquire_upload_lease(uuid, text, integer, integer, integer)",
        "release_upload_lease(uuid)",
        "find_report_receipt(uuid, text)",
        "accept_report(uuid, text, text, uuid, uuid, text, text, text, uuid, text, jsonb, jsonb)",
        "mark_report_storage_ready(uuid, uuid)",
        "pending_report_storage()",
        "staging_upload_is_referenced(uuid)",
        "configure_report_retention(uuid, integer)",
        "sweep_expired_ingestion_state()",
    ];

    for function in functions {
        let statement = format!("GRANT EXECUTE ON FUNCTION {function} TO {quoted_role}");
        sqlx::query(&statement).execute(admin_pool).await?;
    }

    Ok(())
}

async fn clear_pending_bootstrap(admin_pool: &PgPool) -> Result<()> {
    sqlx::query("DELETE FROM bootstrap_credentials WHERE singleton = true")
        .execute(admin_pool)
        .await?;

    Ok(())
}

async fn verify_ingest_role_exists(admin_pool: &PgPool, ingest_role: &str) -> Result<()> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_roles WHERE rolname = $1)")
            .bind(ingest_role)
            .fetch_one(admin_pool)
            .await?;

    if !exists {
        return Err(AppError::Internal(anyhow::anyhow!(
            "reporting database role does not exist"
        )));
    }

    Ok(())
}

fn random_reporting_role() -> String {
    let mut bytes = [0_u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("ladybird_reporting_{}", hex::encode(bytes))
}

fn reporting_database_url(admin_url: &str, role: &str, password: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(admin_url)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("invalid admin database URL")))?;

    url.set_username(role)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("cannot set role in database URL")))?;
    url.set_password(Some(password))
        .map_err(|_| AppError::Internal(anyhow::anyhow!("cannot set password in database URL")))?;

    Ok(url.to_string())
}
