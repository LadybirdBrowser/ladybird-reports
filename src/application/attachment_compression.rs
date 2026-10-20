use crate::infrastructure::{attachments::FileAttachmentStore, database::AdminDatabase};

/// Compresses the text attachments that were stored before text was stored compressed. New ones are compressed as they
/// are accepted, so it looks at each stored one once, when the service starts, and then has nothing left to do.
pub async fn compress_stored_text_attachments(
    database: &AdminDatabase,
    attachments: &FileAttachmentStore,
) {
    let keys = match database.text_attachment_storage_keys().await {
        Ok(keys) => keys,
        Err(error) => {
            tracing::warn!(event = "attachment_compression.listing_failed", ?error);
            return;
        }
    };

    let mut compressed = 0_u64;
    for key in &keys {
        match attachments.compress_stored_text(key).await {
            Ok(true) => compressed += 1,
            Ok(false) => {}
            Err(error) => tracing::warn!(event = "attachment_compression.failed", ?error, key),
        }
    }

    tracing::info!(
        event = "attachment_compression.complete",
        text_attachments = keys.len(),
        compressed,
    );
}
