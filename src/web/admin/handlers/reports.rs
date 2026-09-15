use askama::Template;
use axum::{
    Extension, Form, Json,
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    domain::{AttachmentId, IssueId, ReportId, ReportSearch, filter_expression},
    error::{AppError, Result},
    infrastructure::database::ReportQuery,
};

use super::super::{
    AdminState, TemplateResponse, authentication::Navigation, session::Session,
    templates::not_found,
};

#[derive(Deserialize)]
pub struct ReportFilters {
    #[serde(default = "default_report_search")]
    q: String,
    issue: Option<IssueId>,
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
    before: Option<DateTime<Utc>>,
}

impl Default for ReportFilters {
    fn default() -> Self {
        Self {
            q: default_report_search(),
            issue: None,
            since: None,
            until: None,
            before: None,
        }
    }
}

#[derive(Template)]
#[template(path = "reports/index.html")]
pub struct ReportsTemplate {
    navigation: Option<Navigation>,
    search: String,
    reports: Vec<ReportRow>,
    next_page: Option<String>,
}

pub struct ReportRow {
    id: ReportId,
    client_version: String,
    build: String,
    is_assigned: bool,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct ReportSearchQuery {
    #[serde(default)]
    pub(super) query: String,
}

#[derive(Serialize)]
pub struct EntitySearchResponse {
    pub(super) results: Vec<EntitySearchOption>,
}

#[derive(Serialize)]
pub struct EntitySearchOption {
    pub(super) value: String,
    pub(super) label: String,
    pub(super) description: String,
    pub(super) identifier: String,
    pub(super) badge: String,
    pub(super) badge_tone: &'static str,
    pub(super) footnote: String,
}

#[derive(Template)]
#[template(path = "reports/show.html")]
pub struct ReportTemplate {
    navigation: Option<Navigation>,
    report: ReportView,
    issues: Vec<IssueOption>,
    known_fields: Vec<FieldView>,
    unknown_fields: Vec<FieldView>,
    attachments: Vec<AttachmentView>,
    events: Vec<EventView>,
}

pub struct ReportView {
    id: ReportId,
    client_version: String,
    overview: Vec<OverviewField>,
    is_assigned: bool,
    has_submission_source: bool,
    submission_source_is_blocked: bool,
}

pub struct OverviewField {
    label: &'static str,
    value: String,
    filter_url: Option<String>,
    monospace: bool,
}

pub struct IssueOption {
    id: IssueId,
    title: String,
    is_selected: bool,
}

pub struct FieldView {
    key: String,
    label: String,
    kind: String,
    value: String,
    is_stack: bool,
    is_multiline: bool,
    filter_url: Option<String>,
}

pub struct AttachmentView {
    id: AttachmentId,
    name: String,
    media_type: String,
    size: i64,
    is_png: bool,
}

pub struct EventView {
    action: String,
    actor: String,
    details: String,
    created_at: DateTime<Utc>,
}

pub async fn index(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Query(filters): Query<ReportFilters>,
) -> Result<TemplateResponse<ReportsTemplate>> {
    let query = ReportQuery {
        search: ReportSearch::parse(&filters.q)?,
        issue_id: filters.issue,
        since: filters.since,
        until: filters.until,
        before: filters.before,
    };

    let reports = state.database.list_reports(&query).await?;
    let next_page = next_page_url(&filters, reports.last(), reports.len());
    let report_rows = reports
        .into_iter()
        .map(|report| ReportRow {
            id: report.id,
            client_version: report.client_version,
            build: report.build,
            is_assigned: report.issue_id.is_some(),
            created_at: report.created_at,
        })
        .collect();

    Ok(TemplateResponse(ReportsTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        search: filters.q,
        reports: report_rows,
        next_page,
    }))
}

pub async fn search_options(
    State(state): State<AdminState>,
    Query(parameters): Query<ReportSearchQuery>,
) -> Result<Json<EntitySearchResponse>> {
    let search = parameters.query.trim();

    if search.len() > 128 {
        return Err(AppError::InvalidRequest("Report search is too long"));
    }

    let results = state
        .database
        .search_reports(search)
        .await?
        .into_iter()
        .map(|report| {
            let (badge, badge_tone) = match report.issue_title {
                Some(issue_title) => (format!("Assigned · {issue_title}"), "assigned"),
                None => ("Needs triage".into(), "triage"),
            };
            let description = if report.build.trim().is_empty() {
                report.client_version
            } else {
                format!("{} · {}", report.client_version, report.build)
            };

            EntitySearchOption {
                value: report.id.to_string(),
                label: report_kind_label(&report.kind).into(),
                description,
                identifier: report.id.to_string(),
                badge,
                badge_tone,
                footnote: format!(
                    "Received {}",
                    report.created_at.format("%d %b %Y, %H:%M UTC")
                ),
            }
        })
        .collect();

    Ok(Json(EntitySearchResponse { results }))
}

fn report_kind_label(kind: &str) -> &str {
    match kind {
        "crash" => "Crash report",
        "web_compat" => "Web compatibility report",
        _ => "Diagnostic report",
    }
}

pub async fn show(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
) -> Result<TemplateResponse<ReportTemplate>> {
    let details = state
        .database
        .report_details(report_id)
        .await?
        .ok_or_else(|| not_found("Report not found"))?;
    let issues = state.database.list_issues(false).await?;

    let platform = field_string(&details.fields, "platform").unwrap_or_else(|| "Unknown".into());
    let architecture =
        field_string(&details.fields, "architecture").unwrap_or_else(|| "Unknown".into());

    let source_ip = details
        .report
        .source_ip
        .clone()
        .unwrap_or_else(|| "Unavailable".into());
    let overview = vec![
        OverviewField::searchable(
            "Report type",
            report_kind_label(&details.report.kind),
            "kind",
            &details.report.kind,
        ),
        OverviewField::searchable(
            "Browser version",
            &details.report.client_version,
            "version",
            &details.report.client_version,
        ),
        OverviewField::searchable("Platform", &platform, "platform", &platform),
        OverviewField::searchable("Architecture", &architecture, "architecture", &architecture),
        OverviewField::searchable(
            "Build",
            &details.report.build,
            "build",
            &details.report.build,
        ),
        OverviewField {
            label: "Submitted",
            value: details.report.created_at.to_string(),
            filter_url: Some(submitted_date_filter_url(
                details.report.created_at.date_naive(),
            )),
            monospace: false,
        },
        OverviewField {
            label: "Source IP address",
            value: source_ip.clone(),
            filter_url: details
                .report
                .source_ip
                .as_deref()
                .map(|address| field_filter_url("ip", address)),
            monospace: true,
        },
    ];

    let report_view = ReportView {
        id: details.report.id,
        client_version: details.report.client_version,
        overview,
        is_assigned: details.report.issue_id.is_some(),
        has_submission_source: details.report.has_submission_source,
        submission_source_is_blocked: details.report.submission_source_is_blocked,
    };

    let issue_options = issues
        .into_iter()
        .map(|issue| IssueOption {
            id: issue.id,
            title: issue.title,
            is_selected: details.report.issue_id == Some(issue.id),
        })
        .collect();

    let mut known_fields = Vec::new();
    let mut unknown_fields = Vec::new();

    for field in details.fields {
        let value = field
            .value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| field.value.to_string());
        let known = field.current_label.is_some();
        let key = field.key;

        if matches!(key.as_str(), "platform" | "architecture") {
            continue;
        }

        let view = FieldView {
            label: field.current_label.clone().unwrap_or_else(|| key.clone()),
            is_stack: key == "stack",
            is_multiline: field.kind == "multiline",
            filter_url: (field.kind != "multiline").then(|| field_filter_url(&key, &value)),
            kind: field.kind,
            key,
            value,
        };

        if known {
            known_fields.push(view);
        } else {
            unknown_fields.push(view);
        }
    }

    let attachments = details
        .attachments
        .into_iter()
        .map(|attachment| AttachmentView {
            id: attachment.id,
            name: attachment.name,
            is_png: attachment.media_type == "image/png",
            media_type: attachment.media_type,
            size: attachment.size,
        })
        .collect();

    let events = details
        .events
        .into_iter()
        .map(|event| EventView {
            action: event.action,
            actor: event.actor_login.unwrap_or_else(|| "system".into()),
            details: event.details.to_string(),
            created_at: event.created_at,
        })
        .collect();

    Ok(TemplateResponse(ReportTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        report: report_view,
        issues: issue_options,
        known_fields,
        unknown_fields,
        attachments,
        events,
    }))
}

impl OverviewField {
    fn searchable(label: &'static str, value: &str, key: &str, search_value: &str) -> Self {
        Self {
            label,
            value: value.into(),
            filter_url: Some(field_filter_url(key, search_value)),
            monospace: false,
        }
    }
}

#[derive(Deserialize)]
pub struct AssignmentForm {
    csrf: String,
    report_ids: String,
    issue_id: String,
}

pub async fn assign(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Form(form): Form<AssignmentForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;

    let report_ids = parse_report_ids(&form.report_ids)?;
    let issue_id = if form.issue_id.trim().is_empty() {
        None
    } else {
        Some(
            form.issue_id
                .trim()
                .parse::<IssueId>()
                .map_err(|_| AppError::InvalidRequest("Invalid issue ID"))?,
        )
    };

    state
        .database
        .assign_reports(&report_ids, issue_id, session.github_id)
        .await?;

    tracing::info!(
        event = "reports.assignment_updated",
        report_count = report_ids.len(),
        issue_id = issue_id.map(|id| id.to_string()),
        actor = session.login,
    );

    Ok(Redirect::to("/"))
}

#[derive(Deserialize)]
pub struct SourceRateLimitForm {
    csrf: String,
    #[serde(default)]
    remove_triage_reports: bool,
}

pub async fn block_ip(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
    Form(form): Form<SourceRateLimitForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    let outcome = state
        .database
        .block_report_source(report_id, session.github_id, form.remove_triage_reports)
        .await?;

    tracing::warn!(
        event = "report.source_blocked",
        %report_id,
        removed_triage_reports = outcome.removed_triage_reports,
        actor = session.login,
    );

    if outcome.current_report_removed {
        Ok(Redirect::to("/"))
    } else {
        Ok(Redirect::to(&format!("/reports/{report_id}")))
    }
}

pub async fn unblock_ip(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
    Form(form): Form<SourceRateLimitForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    state
        .database
        .unblock_report_source(report_id, session.github_id)
        .await?;

    tracing::warn!(
        event = "report.source_unblocked",
        %report_id,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/reports/{report_id}")))
}

pub async fn attachment(
    State(state): State<AdminState>,
    Path(attachment_id): Path<AttachmentId>,
) -> Result<Response> {
    let attachment = state
        .database
        .attachment(attachment_id)
        .await?
        .ok_or(AppError::NotFound("Attachment not found"))?;

    let bytes = state.attachments.read(&attachment.storage_key).await?;
    let content_type = if attachment.media_type == "image/png" {
        "image/png"
    } else {
        "text/plain; charset=utf-8"
    };

    Ok((
        [
            ("content-type", content_type),
            ("content-disposition", "inline"),
        ],
        bytes,
    )
        .into_response())
}

pub fn parse_report_ids(value: &str) -> Result<Vec<ReportId>> {
    let report_ids = value
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<ReportId>()
                .map_err(|_| AppError::InvalidRequest("Invalid report ID"))
        })
        .collect::<Result<Vec<_>>>()?;

    if report_ids.len() > 100 {
        return Err(AppError::InvalidRequest("Select at most 100 reports"));
    }

    Ok(report_ids)
}

fn next_page_url(
    filters: &ReportFilters,
    last: Option<&crate::infrastructure::database::ReportSummary>,
    result_count: usize,
) -> Option<String> {
    if result_count != 100 {
        return None;
    }

    let last = last?;
    let mut url = reqwest::Url::parse("http://localhost/").expect("static URL is valid");

    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("q", &filters.q)
            .append_pair("before", &last.created_at.to_rfc3339());

        if let Some(issue_id) = filters.issue {
            query.append_pair("issue", &issue_id.to_string());
        }
        if let Some(since) = filters.since {
            query.append_pair("since", &since.to_string());
        }
        if let Some(until) = filters.until {
            query.append_pair("until", &until.to_string());
        }
    }

    Some(format!("/?{}", url.query().unwrap_or_default()))
}

fn default_report_search() -> String {
    "state:triage".into()
}

fn field_string(
    fields: &[crate::infrastructure::database::StoredDiagnosticField],
    key: &str,
) -> Option<String> {
    fields
        .iter()
        .find(|field| field.key == key)
        .and_then(|field| field.value.as_str())
        .map(str::to_owned)
}

fn field_filter_url(key: &str, value: &str) -> String {
    let mut url = reqwest::Url::parse("http://localhost/").expect("static URL is valid");
    url.query_pairs_mut()
        .append_pair("q", &format!("state:all {}", filter_expression(key, value)));

    format!("/?{}", url.query().unwrap_or_default())
}

fn submitted_date_filter_url(date: NaiveDate) -> String {
    let mut url = reqwest::Url::parse("http://localhost/").expect("static URL is valid");
    url.query_pairs_mut()
        .append_pair("q", "state:all")
        .append_pair("since", &date.to_string())
        .append_pair("until", &date.to_string());

    format!("/?{}", url.query().unwrap_or_default())
}
