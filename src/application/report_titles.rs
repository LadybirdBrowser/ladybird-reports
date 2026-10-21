use crate::{
    domain::{ReportTitleInput, TitleStyle, generate_report_title},
    infrastructure::database::{ReportDetails, StoredDiagnosticField},
};

/// The title of a report, as it is shown in this service.
pub fn title_for_report(details: &ReportDetails) -> String {
    title_in_style(details, TitleStyle::Report)
}

/// The title proposed for a GitHub issue about a report.
pub fn issue_title_for_report(details: &ReportDetails) -> String {
    title_in_style(details, TitleStyle::Issue)
}

fn title_in_style(details: &ReportDetails, style: TitleStyle) -> String {
    let text_field = |key| field_text(&details.fields, key);
    let stack_trace = stack_trace_for_report(details);

    generate_report_title(ReportTitleInput {
        style,
        kind: details.report.kind,
        url: text_field("url"),
        client_version: &details.report.client_version,
        failure_reason: text_field("failure_reason"),
        stack_trace,
        process: text_field("process"),
        platform: text_field("platform"),
        signal: text_field("signal"),
    })
}

pub(crate) fn stack_trace_for_report(details: &ReportDetails) -> Option<&str> {
    details
        .fields
        .iter()
        .filter(|field| field.is_stack_trace())
        .filter_map(|field| field.value.as_str().map(|value| (field, value)))
        .min_by_key(|(field, _)| field.key != "stack")
        .map(|(_, value)| value)
}

fn field_text<'a>(fields: &'a [StoredDiagnosticField], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|field| field.key == key)
        .and_then(|field| field.value.as_str())
}
