mod admin;
mod configuration_cache;
mod ingest;
mod permissions;
mod proxy_addresses;

pub use admin::*;
pub use configuration_cache::*;
pub use ingest::*;
pub use permissions::*;

use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::error::Result;

/// Connections kept open while idle, so a request does not have to wait for one
/// to be opened. One goes to the readiness check, which runs every few seconds.
const MINIMUM_CONNECTIONS: u32 = 2;
const MAXIMUM_CONNECTIONS: u32 = 16;

/// The pool settings of both services.
fn pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .min_connections(MINIMUM_CONNECTIONS)
        .max_connections(MAXIMUM_CONNECTIONS)
}

async fn connect_pool(database_url: &str) -> Result<PgPool> {
    Ok(pool_options().connect(database_url).await?)
}
