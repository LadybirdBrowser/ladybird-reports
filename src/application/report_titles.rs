use crate::{
    domain::{ReportTitleInput, generate_report_title},
    infrastructure::database::{ReportDetails, StoredDiagnosticField},
};

pub fn title_for_report(details: &ReportDetails) -> String {
    let text_field = |key| field_text(&details.fields, key);
    let stack_trace = details
        .fields
        .iter()
        .filter(|field| {
            field.kind == "stack_trace"
                || field.current_kind.as_deref() == Some("stack_trace")
                || (field.key == "stack" && field.kind == "multiline")
        })
        .min_by_key(|field| field.key != "stack")
        .and_then(|field| field.value.as_str());

    generate_report_title(ReportTitleInput {
        kind: &details.report.kind,
        client_version: &details.report.client_version,
        stack_trace,
        process: text_field("process"),
        platform: text_field("platform"),
    })
}

fn field_text<'a>(fields: &'a [StoredDiagnosticField], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|field| field.key == key)
        .and_then(|field| field.value.as_str())
}
