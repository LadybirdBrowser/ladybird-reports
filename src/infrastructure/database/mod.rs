mod admin;
mod configuration_cache;
mod ingest;
mod migrations;
mod permissions;

pub use admin::*;
pub use configuration_cache::*;
pub use ingest::*;
pub use migrations::*;
pub use permissions::*;

use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::error::Result;

async fn connect_pool(database_url: &str, maximum_connections: u32) -> Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(maximum_connections)
        .connect(database_url)
        .await?)
}
