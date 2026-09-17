use crate::infrastructure::database::ReportDetails;

use super::title_for_report;

pub struct IssueProposal {
    pub title: String,
    pub description: String,
}

/// Prepare a public GitHub draft from a small, deliberate set of report data.
/// URLs, raw stack traces, unknown fields, and attachments stay in Reports.
/// The proposed title uses only the concise function name derived from a stack.
pub fn propose_issue(details: &ReportDetails) -> IssueProposal {
    let report_type = match details.report.kind.as_str() {
        "crash" => "Crash",
        "web_compat" => "Web compatibility issue",
        _ => "Diagnostic issue",
    };
    let platform = field_value(details, "platform", 40);
    let title = title_for_report(details);

    let mut description =
        format!("## Summary\n\n{report_type} reported by Ladybird.\n\n## Environment\n");
    add_detail(
        &mut description,
        "Ladybird version",
        safe_value(&details.report.client_version, 100),
    );
    add_detail(
        &mut description,
        "Build",
        safe_value(&details.report.build, 100),
    );
    add_detail(&mut description, "Platform", platform);
    add_detail(
        &mut description,
        "Architecture",
        field_value(details, "architecture", 40),
    );
    add_detail(
        &mut description,
        "Signal",
        field_value(details, "signal", 40),
    );

    IssueProposal { title, description }
}

fn field_value(details: &ReportDetails, key: &str, maximum_characters: usize) -> Option<String> {
    details
        .fields
        .iter()
        .find(|field| field.key == key && field.kind == "text")
        .and_then(|field| field.value.as_str())
        .and_then(|value| safe_value(value, maximum_characters))
}

fn safe_value(value: &str, maximum_characters: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > maximum_characters
        || value.contains("://")
        || value.chars().any(|character| {
            !character.is_alphanumeric()
                && !matches!(
                    character,
                    ' ' | '.' | '_' | '-' | '+' | '(' | ')' | '/' | '·' | ':' | ','
                )
        })
    {
        return None;
    }

    Some(value.to_owned())
}

fn add_detail(description: &mut String, label: &str, value: Option<String>) {
    if let Some(value) = value {
        description.push_str(&format!("- {label}: {}\n", value.replace('_', "\\_")));
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::{
        domain::{ReportId, SubmissionId},
        infrastructure::database::{ReportRecord, StoredDiagnosticField},
    };

    #[test]
    fn public_proposal_excludes_private_and_unrecognized_fields() {
        let details = ReportDetails {
            report: ReportRecord {
                id: ReportId::new(),
                submission_id: SubmissionId::new(),
                manifest_digest: String::new(),
                kind: "crash".into(),
                client_version: "Ladybird Nightly 2026.09.16".into(),
                build: "Release".into(),
                issue_id: None,
                confirmed_at: None,
                source_ip: Some("192.0.2.1".into()),
                has_submission_source: true,
                submission_source_is_blocked: false,
                created_at: Utc::now(),
                expires_at: None,
            },
            fields: vec![
                field("platform", "text", json!("macOS")),
                field("architecture", "text", json!("arm64")),
                field("signal", "text", json!("SIGABRT")),
                field("url", "text", json!("https://private.example/path")),
                field("stack", "multiline", json!("private stack trace")),
                field("unknown", "text", json!("private unknown field")),
            ],
            attachments: Vec::new(),
            events: Vec::new(),
        };

        let proposal = propose_issue(&details);
        assert_eq!(proposal.title, "Crash report · macOS");
        assert!(proposal.description.contains("- Platform: macOS"));
        assert!(proposal.description.contains("- Signal: SIGABRT"));
        for private_value in [
            "private.example",
            "private stack",
            "192.0.2.1",
            "private unknown",
        ] {
            assert!(!proposal.description.contains(private_value));
        }
        assert!(safe_value("@someone", 40).is_none());
        assert!(safe_value("macOS\nprivate data", 40).is_none());
    }

    fn field(key: &str, kind: &str, value: serde_json::Value) -> StoredDiagnosticField {
        StoredDiagnosticField {
            key: key.into(),
            kind: kind.into(),
            value,
            recognized_at_submission: true,
            current_label: None,
            current_kind: None,
            current_position: None,
        }
    }
}
