use crate::infrastructure::database::ReportDetails;

use super::{stack_trace_for_report, title_for_report};

pub struct IssueProposal {
    pub title: String,
    pub description: String,
}

/// Prepare a public GitHub draft from a small, deliberate set of report data.
/// The raw stack trace is included; the URL field, unknown fields, and
/// attachments stay in Reports unless a maintainer adds them to the draft.
/// The proposed title uses only the concise function name derived from a stack.
pub fn propose_issue(details: &ReportDetails) -> IssueProposal {
    let report_type = match details.report.kind.as_str() {
        "crash" => "Crash",
        "web_compat" => "Web compatibility issue",
        _ => "Diagnostic issue",
    };
    let platform = field_value(details, "platform", 40);
    let title = title_for_report(details);

    let mut description = format!("## Summary\n\n{report_type} reported by Ladybird.\n");

    if let Some(stack_trace) = stack_trace_for_report(details).filter(|stack| !stack.is_empty()) {
        add_stack_trace(&mut description, stack_trace);
    }

    description.push_str("\n## Environment\n\n");
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

fn add_stack_trace(description: &mut String, stack_trace: &str) {
    // A trace may itself contain Markdown fences. Use a longer fence so the
    // whole stored value remains inside one code block.
    let longest_run = stack_trace
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_run.saturating_add(1).max(3));

    description.push_str("\n## Stack trace\n\n");
    description.push_str(&fence);
    description.push_str("text\n");
    description.push_str(stack_trace);
    if !stack_trace.ends_with('\n') {
        description.push('\n');
    }
    description.push_str(&fence);
    description.push('\n');
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
    fn public_proposal_includes_stack_but_excludes_other_private_fields() {
        let details = ReportDetails {
            report: ReportRecord {
                id: ReportId::new(),
                submission_id: SubmissionId::new(),
                manifest_digest: String::new(),
                kind: "crash".into(),
                client_version: "Ladybird Nightly 2026.09.16".into(),
                build: "Release".into(),
                issue_id: None,
                state: "triage".into(),
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
        assert!(
            proposal
                .description
                .contains("## Stack trace\n\n```text\nprivate stack trace\n```")
        );
        for private_value in ["private.example", "192.0.2.1", "private unknown"] {
            assert!(!proposal.description.contains(private_value));
        }
        assert!(safe_value("@someone", 40).is_none());
        assert!(safe_value("macOS\nprivate data", 40).is_none());
    }

    #[test]
    fn stack_trace_with_markdown_fence_stays_in_one_code_block() {
        let mut description = String::new();
        add_stack_trace(&mut description, "first frame\n```\nsecond frame");
        assert_eq!(
            description,
            "\n## Stack trace\n\n````text\nfirst frame\n```\nsecond frame\n````\n"
        );
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
