use crate::{
    error::Result,
    infrastructure::{attachments::FileAttachmentStore, database::AdminDatabase},
};

const FAILURE_PREFIXES: [&str; 3] = [
    "Verification failed: ",
    "Assertion failed: ",
    "Rust panic: ",
];

pub fn failure_reason_from_diagnostics(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let header = text
        .split("\nNative stack (binary build ID, object address):")
        .next()?;

    header.lines().find_map(|line| {
        let line = line.trim_end_matches('\r');
        if line.len() > 4096
            || !FAILURE_PREFIXES
                .iter()
                .any(|prefix| line.starts_with(prefix))
        {
            return None;
        }

        if let Some((message, location)) = line.rsplit_once(" at ") {
            let location = sanitize_location(location);
            return (!location.is_empty()).then(|| format!("{message} at {location}"));
        }

        Some(line.to_owned())
    })
}

fn sanitize_location(location: &str) -> &str {
    if !location.starts_with('/') && !location.contains('\\') {
        return location;
    }

    for root in ["AK/", "Libraries/", "Services/", "Tests/", "UI/"] {
        if let Some(position) = location.find(&format!("/{root}")) {
            return &location[position + 1..];
        }
    }

    location.rsplit(['/', '\\']).next().unwrap_or(location)
}

pub async fn backfill_failure_reasons(
    database: &AdminDatabase,
    attachments: &FileAttachmentStore,
) -> Result<usize> {
    let pending = database.pending_failure_reasons().await?;

    for attachment in &pending {
        let reason = match attachments.read(&attachment.storage_key).await {
            Ok(bytes) => failure_reason_from_diagnostics(&bytes),
            Err(error) => {
                tracing::warn!(
                    event = "failure_reason.attachment_unavailable",
                    report_id = %attachment.report_id,
                    ?error,
                );
                None
            }
        };
        database
            .finish_failure_reason(attachment, reason.as_deref())
            .await?;
    }

    Ok(pending.len())
}

#[cfg(test)]
mod tests {
    use super::failure_reason_from_diagnostics;

    #[test]
    fn extracts_sanitized_failure_before_the_stack() {
        let diagnostic = b"Ladybird crash report, format 1\n\
            Verification failed: false at /Users/jelle/Projects/ladybird/Libraries/LibMedia/FFmpeg/FFmpegVideoDecoder.cpp:235\n\
            Native stack (binary build ID, object address):\n#0 frame\n";

        assert_eq!(
            failure_reason_from_diagnostics(diagnostic).as_deref(),
            Some(
                "Verification failed: false at Libraries/LibMedia/FFmpeg/FFmpegVideoDecoder.cpp:235"
            )
        );
    }

    #[test]
    fn unrelated_diagnostics_have_no_failure_reason() {
        assert_eq!(
            failure_reason_from_diagnostics(
                b"Ladybird crash report, format 1\nProcess: WebContent\n"
            ),
            None
        );
    }
}
