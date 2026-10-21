use crate::{
    domain::{FieldKind, ReportKind},
    infrastructure::database::{ReportDetails, StoredAttachment},
};

use super::{stack_trace_for_report, title_for_report};

/// Stands for the report's screenshot in an issue description. When the issue is created, the screenshot is uploaded
/// to GitHub and this is replaced by the image, so a maintainer can remove it to leave the screenshot out.
pub const SCREENSHOT_PLACEHOLDER: &str = "{screenshot}";

pub struct IssueProposal {
    pub title: String,
    pub description: String,
}

/// Prepare a public GitHub draft from a small, deliberate set of report data. The raw stack trace is included; the URL
/// page URL is included; unknown fields and attachments stay in Reports unless a
/// maintainer adds them to the draft, except for a screenshot, which the draft refers to with a placeholder. The proposed title uses only the concise function name derived from a stack.
pub fn propose_issue(details: &ReportDetails) -> IssueProposal {
    let report_type = match details.report.kind {
        ReportKind::Crash => "Crash",
        ReportKind::WebCompat => "Web compatibility issue",
    };
    let platform = field_value(details, "platform", 40);
    let title = title_for_report(details);

    let mut description = format!("## Summary\n\n{report_type} reported by a Ladybird user.\n");

    if screenshot_attachment(&details.attachments).is_some() {
        description.push_str(&format!("\n## Screenshot\n\n{SCREENSHOT_PLACEHOLDER}\n"));
    }

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

    if let Some(page_url) = page_url(details) {
        description.push_str(&format!("- Page URL: `{page_url}`\n"));
    }

    IssueProposal { title, description }
}

/// The screenshot of a report: the first PNG by name, as shown with the report.
pub fn screenshot_attachment(attachments: &[StoredAttachment]) -> Option<&StoredAttachment> {
    attachments
        .iter()
        .filter(|attachment| attachment.media_type == "image/png")
        .min_by_key(|attachment| attachment.name.to_lowercase())
}

/// Replace the screenshot placeholder with the uploaded image.
pub fn embed_screenshot(description: &str, url: &str) -> String {
    description.replace(
        SCREENSHOT_PLACEHOLDER,
        &format!("![Screenshot of the page in the report]({url})"),
    )
}

/// The page a report came from. It is meant to be shown as code so that GitHub does not turn an address chosen by an
/// anonymous user into a link.
fn page_url(details: &ReportDetails) -> Option<String> {
    let url = details
        .fields
        .iter()
        .find(|field| field.key == "url" && matches!(field.kind, FieldKind::Text | FieldKind::Url))?
        .value
        .as_str()?;
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    Some(
        url.chars()
            .map(|character| match character {
                '`' => '′',
                character if character.is_control() => ' ',
                character => character,
            })
            .collect(),
    )
}

fn add_stack_trace(description: &mut String, stack_trace: &str) {
    // A trace may itself contain Markdown fences. Use a longer fence so the whole stored value remains inside one code
    // block.
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
        .find(|field| field.key == key && field.kind == FieldKind::Text)
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
        domain::{AttachmentId, ReportId, ReportState},
        infrastructure::database::{ReportRecord, StoredDiagnosticField},
    };

    #[test]
    fn public_proposal_includes_stack_and_page_url_but_excludes_other_private_fields() {
        let details = ReportDetails {
            report: ReportRecord {
                id: ReportId::new(),
                kind: ReportKind::Crash,
                client_version: "Ladybird Nightly 2026.09.16".into(),
                build: "Release".into(),
                issue_id: None,
                state: ReportState::Triage,
                has_submission_source: true,
                submission_source_is_blocked: false,
                created_at: Utc::now(),
                expires_at: None,
            },
            fields: vec![
                field("platform", FieldKind::Text, json!("macOS")),
                field("architecture", FieldKind::Text, json!("arm64")),
                field("signal", FieldKind::Text, json!("SIGABRT")),
                field(
                    "url",
                    FieldKind::Url,
                    json!("https://example.test/path?token=secret#section"),
                ),
                field("stack", FieldKind::Multiline, json!("private stack trace")),
                field("unknown", FieldKind::Text, json!("private unknown field")),
            ],
            attachments: Vec::new(),
            events: Vec::new(),
        };

        let proposal = propose_issue(&details);
        assert_eq!(proposal.title, "Crash: macOS · SIGABRT");
        assert!(
            proposal
                .description
                .starts_with("## Summary\n\nCrash reported by a Ladybird user.\n")
        );
        assert!(proposal.description.contains("- Platform: macOS"));
        assert!(proposal.description.contains("- Signal: SIGABRT"));
        assert!(
            proposal
                .description
                .contains("- Page URL: `https://example.test/path?token=secret#section`\n")
        );
        assert!(
            proposal
                .description
                .contains("## Stack trace\n\n```text\nprivate stack trace\n```")
        );
        for private_value in ["192.0.2.1", "private unknown"] {
            assert!(!proposal.description.contains(private_value));
        }
        assert!(safe_value("@someone", 40).is_none());
        assert!(safe_value("macOS\nprivate data", 40).is_none());
    }

    #[test]
    fn screenshot_is_the_first_png_by_name_and_replaces_its_placeholder() {
        let attachment = |name: &str, media_type: &str| StoredAttachment {
            id: AttachmentId::new(),
            name: name.into(),
            media_type: media_type.into(),
            size: 1,
            storage_key: name.into(),
        };
        let attachments = [
            attachment("crash-diagnostics.txt", "text/plain"),
            attachment("b.png", "image/png"),
            attachment("A.png", "image/png"),
        ];

        assert_eq!(screenshot_attachment(&attachments).unwrap().name, "A.png");
        assert!(screenshot_attachment(&attachments[..1]).is_none());
        assert_eq!(
            embed_screenshot(
                "Before\n\n{screenshot}\n",
                "https://github.com/user-attachments/assets/abc"
            ),
            "Before\n\n![Screenshot of the page in the report](https://github.com/user-attachments/assets/abc)\n"
        );
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

    fn field(key: &str, kind: FieldKind, value: serde_json::Value) -> StoredDiagnosticField {
        StoredDiagnosticField {
            key: key.into(),
            kind,
            value,
            recognized_at_submission: true,
            current_label: None,
            current_kind: None,
        }
    }
}
