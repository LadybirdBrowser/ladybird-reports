use std::time::Duration;

use crate::{error::Result, infrastructure::database::AdminDatabase};

const BACKLOG_PAUSE: Duration = Duration::from_millis(100);
const RETRY_DELAY: Duration = Duration::from_secs(30);

/// Keeps stack trace signatures current for as long as the process runs.
///
/// It waits for the database to announce work instead of polling for it. On
/// start, and again whenever its connection or an indexing run fails, it first
/// scans for traces that were never indexed, so a report that became ready while
/// the service was down or disconnected is not left behind.
pub async fn run_stack_indexer(database: AdminDatabase) {
    loop {
        if let Err(error) = index_until_it_fails(&database).await {
            tracing::warn!(event = "stack_index.failed", ?error);
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }
}

async fn index_until_it_fails(database: &AdminDatabase) -> Result<()> {
    // Subscribe before scanning, so nothing that arrives during the scan is missed.
    let mut listener = database.listen_for_stack_index_work().await?;

    let count = database.drain_pending_stack_traces(BACKLOG_PAUSE).await?;
    tracing::info!(event = "stack_index.startup_check_complete", count);

    loop {
        // `None` means the connection dropped and was re-established, so
        // announcements may have been lost; scanning again covers them.
        if listener.try_recv().await?.is_none() {
            tracing::warn!(event = "stack_index.listener_reconnected");
        }

        // Announcements already queued need no scans of their own: the scan that
        // follows sees everything they refer to.
        while let Ok(Ok(Some(_))) = tokio::time::timeout(Duration::ZERO, listener.try_recv()).await
        {
        }

        let count = database.drain_pending_stack_traces(BACKLOG_PAUSE).await?;
        if count > 0 {
            tracing::info!(event = "stack_index.batch_complete", count);
        }
    }
}
