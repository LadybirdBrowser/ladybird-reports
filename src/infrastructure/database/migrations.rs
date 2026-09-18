use sqlx::{PgPool, Row, migrate::Migrator};

use crate::error::{AppError, Result};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

// Migration 25 is the last migration in the original history. On the first
// boot with the consolidated baseline, replace that history with its single
// version-25 entry. This preserves the production schema version and data.
const PREVIOUS_MIGRATION_25_CHECKSUM: &str = "44ee091e94b7360a7e9dd17734d9938cbcb1289e2a732ac3298e7b3ddf47dff5a27eed7b9d941f156c1fc68257195643";

pub async fn migrate_database(pool: &PgPool) -> Result<()> {
    consolidate_previous_history(pool).await?;
    MIGRATOR
        .run(pool)
        .await
        .map_err(|error| AppError::Internal(error.into()))?;

    tracing::info!(event = "database.migrations_complete");
    Ok(())
}

async fn consolidate_previous_history(pool: &PgPool) -> Result<()> {
    let exists: bool =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if !exists {
        return Ok(());
    }

    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(20260918)")
        .execute(&mut *transaction)
        .await?;

    let rows = sqlx::query(
        "SELECT version, checksum, success
         FROM _sqlx_migrations
         ORDER BY version
         FOR UPDATE",
    )
    .fetch_all(&mut *transaction)
    .await?;

    let previous_checksum = hex::decode(PREVIOUS_MIGRATION_25_CHECKSUM)
        .map_err(|error| AppError::Internal(error.into()))?;
    let is_previous_history = rows.len() == 25
        && rows.iter().enumerate().all(|(index, row)| {
            row.get::<i64, _>("version") == index as i64 + 1 && row.get::<bool, _>("success")
        })
        && rows[24].get::<Vec<u8>, _>("checksum") == previous_checksum;

    if is_previous_history {
        let baseline = MIGRATOR
            .iter()
            .find(|migration| migration.version == 25)
            .expect("version 25 baseline exists");

        sqlx::query("DELETE FROM _sqlx_migrations WHERE version < 25")
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "UPDATE _sqlx_migrations
             SET description = $1, checksum = $2
             WHERE version = 25",
        )
        .bind(baseline.description.as_ref())
        .bind(baseline.checksum.as_ref())
        .execute(&mut *transaction)
        .await?;

        tracing::info!(
            event = "database.migration_history_consolidated",
            version = 25
        );
    }

    transaction.commit().await?;
    Ok(())
}
