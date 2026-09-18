use askama::Template;
use axum::{
    Extension, Form, Json,
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    application::{IssueProposal, propose_issue, title_for_report},
    domain::{
        AttachmentId, IssueId, ParsedStackTrace, ReportId, ReportSearch, filter_expression,
        parse_stack_trace, stack_fingerprint,
    },
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
    before_id: Option<ReportId>,
}

impl Default for ReportFilters {
    fn default() -> Self {
        Self {
            q: default_report_search(),
            issue: None,
            since: None,
            until: None,
            before: None,
            before_id: None,
        }
    }
}

#[derive(Template)]
#[template(path = "reports/index.html")]
pub struct ReportsTemplate {
    navigation: Option<Navigation>,
    asset_version: &'static str,
    search: String,
    reports: Vec<ReportRow>,
    next_page: Option<String>,
}

#[derive(Template)]
#[template(path = "reports/_list.html")]
pub struct ReportListTemplate {
    reports: Vec<ReportRow>,
    next_page: Option<String>,
}

pub struct ReportRow {
    id: ReportId,
    title: String,
    metadata: String,
    state_label: &'static str,
    state_tone: &'static str,
    received_at: String,
}

#[derive(Deserialize)]
pub struct ReportSearchQuery {
    #[serde(default)]
    pub(super) query: String,
}

#[derive(Deserialize)]
pub struct ReportCompletionQuery {
    #[serde(default)]
    token: String,
}

#[derive(Serialize)]
pub struct ReportCompletionResponse {
    results: Vec<ReportCompletion>,
}

#[derive(Serialize)]
pub struct ReportCompletion {
    replacement: String,
    label: String,
    description: String,
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
    pub(super) group: Option<&'static str>,
}

#[derive(Template)]
#[template(path = "reports/show.html")]
pub struct ReportTemplate {
    navigation: Option<Navigation>,
    asset_version: &'static str,
    report: ReportView,
    linked_issue: Option<LinkedIssueView>,
    potential_issues: Vec<PotentialIssueView>,
    issue_proposal: IssueProposal,
    known_groups: Vec<FieldGroup>,
    unknown_groups: Vec<FieldGroup>,
    attachments: Vec<AttachmentView>,
    events: Vec<EventView>,
}

pub struct ReportView {
    id: ReportId,
    title: String,
    overview: Vec<OverviewField>,
    is_assigned: bool,
    is_confirmed: bool,
    is_rejected: bool,
    has_submission_source: bool,
    submission_source_is_blocked: bool,
}

pub struct LinkedIssueView {
    id: IssueId,
    title: String,
    github_number: i64,
    github_url: String,
}

pub struct PotentialIssueView {
    id: IssueId,
    title: String,
    github_state: String,
}

pub struct OverviewField {
    label: &'static str,
    value: String,
    filter_url: Option<String>,
    full_width: bool,
}

pub struct FieldView {
    key: String,
    label: String,
    kind: String,
    value: String,
    is_multiline: bool,
    stack: Option<ParsedStackTrace>,
    stack_signature: Option<StackSignatureView>,
    filter_url: Option<String>,
    known: bool,
}

pub struct FieldGroup {
    inline: bool,
    fields: Vec<FieldView>,
}

pub struct StackSignatureView {
    short: String,
    full: String,
}

pub struct AttachmentView {
    id: AttachmentId,
    name: String,
    media_type: String,
    size: String,
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
    let list = load_report_list(&state, &filters).await?;

    Ok(TemplateResponse(ReportsTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        asset_version: super::assets::asset_version(),
        search: filters.q,
        reports: list.reports,
        next_page: list.next_page,
    }))
}

pub async fn list(
    State(state): State<AdminState>,
    Query(filters): Query<ReportFilters>,
) -> Result<TemplateResponse<ReportListTemplate>> {
    Ok(TemplateResponse(load_report_list(&state, &filters).await?))
}

async fn load_report_list(
    state: &AdminState,
    filters: &ReportFilters,
) -> Result<ReportListTemplate> {
    if filters.before.is_some() != filters.before_id.is_some() {
        return Err(AppError::InvalidRequest("Incomplete report cursor"));
    }

    let query = ReportQuery {
        search: ReportSearch::parse(&filters.q)?,
        issue_id: filters.issue,
        since: filters.since,
        until: filters.until,
        before: filters.before,
        before_id: filters.before_id,
    };

    let mut reports = state.database.list_reports(&query).await?;
    let has_more = reports.len() > 50;
    reports.truncate(50);
    let next_page = has_more
        .then(|| next_page_url(filters, reports.last()))
        .flatten();
    let reports = reports
        .into_iter()
        .map(|report| {
            let (state_label, state_tone) = report_state(&report.state);

            ReportRow {
                id: report.id,
                title: report.title,
                metadata: report_metadata(
                    report.platform.as_deref(),
                    report.architecture.as_deref(),
                    &report.client_version,
                ),
                state_label,
                state_tone,
                received_at: report.created_at.format("%d %b %Y, %H:%M UTC").to_string(),
            }
        })
        .collect();

    Ok(ReportListTemplate { reports, next_page })
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
            let (state_label, badge_tone) = report_state(&report.state);
            let badge = report
                .issue_title
                .map(|title| format!("{state_label} · {title}"))
                .unwrap_or_else(|| state_label.into());
            let description = report_metadata(
                report.platform.as_deref(),
                report.architecture.as_deref(),
                &report.client_version,
            );

            EntitySearchOption {
                value: report.id.to_string(),
                label: report.title,
                description,
                identifier: report.id.to_string(),
                badge,
                badge_tone,
                footnote: format!(
                    "Received {}",
                    report.created_at.format("%d %b %Y, %H:%M UTC")
                ),
                group: None,
            }
        })
        .collect();

    Ok(Json(EntitySearchResponse { results }))
}

pub async fn search_completions(
    State(state): State<AdminState>,
    Query(parameters): Query<ReportCompletionQuery>,
) -> Result<Json<ReportCompletionResponse>> {
    let token = parameters.token.trim();
    if token.len() > 128 {
        return Err(AppError::InvalidRequest("Search token is too long"));
    }

    let Some((key, value_prefix)) = token.split_once(':') else {
        let prefix = token.to_ascii_lowercase();
        let mut keys = vec![
            ("state", "Workflow state"),
            ("kind", "Report type"),
            ("version", "Browser version"),
            ("build", "Build description"),
            ("id", "Report ID"),
        ];
        let definitions = state.database.field_definitions().await?;
        for definition in &definitions {
            if !matches!(
                definition.kind.as_str(),
                "multiline" | "stack_trace" | "attachment"
            ) {
                keys.push((&definition.key, &definition.label));
            }
        }
        keys.sort_unstable_by(|left, right| left.0.cmp(right.0));
        keys.dedup_by(|left, right| left.0 == right.0);

        let results = keys
            .into_iter()
            .filter(|(key, _)| key.to_ascii_lowercase().starts_with(&prefix))
            .take(12)
            .map(|(key, description)| ReportCompletion {
                replacement: format!("{key}:"),
                label: format!("{key}:"),
                description: description.to_owned(),
            })
            .collect();

        return Ok(Json(ReportCompletionResponse { results }));
    };

    let key = key.to_ascii_lowercase();
    let prefix = value_prefix.trim_matches('"').to_ascii_lowercase();
    let values = match key.as_str() {
        "state" => vec!["triage", "confirmed", "rejected"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        "kind" => vec!["crash", "web_compat"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        "id" | "report" => Vec::new(),
        _ => {
            let values = state.database.report_search_values(&key).await?;
            if values.len() > 25 {
                Vec::new()
            } else {
                values
            }
        }
    };
    let results = values
        .into_iter()
        .filter(|value| value.to_ascii_lowercase().starts_with(&prefix))
        .take(12)
        .map(|value| ReportCompletion {
            replacement: filter_expression(&key, &value),
            label: value,
            description: format!("Value for {key}"),
        })
        .collect();

    Ok(Json(ReportCompletionResponse { results }))
}

fn report_kind_label(kind: &str) -> &str {
    match kind {
        "crash" => "Crash report",
        "web_compat" => "Web compatibility report",
        _ => "Diagnostic report",
    }
}

fn report_metadata(platform: Option<&str>, architecture: Option<&str>, version: &str) -> String {
    [platform, architecture, Some(version)]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(64).collect::<String>())
        .collect::<Vec<_>>()
        .join(" · ")
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
    state.database.index_report_stack_traces(report_id).await?;
    let potential_issues = if details.report.issue_id.is_none() {
        state
            .database
            .potential_issue_matches(report_id)
            .await?
            .into_iter()
            .map(|issue| PotentialIssueView {
                id: issue.id,
                title: issue.title,
                github_state: issue.github_state,
            })
            .collect()
    } else {
        Vec::new()
    };
    let linked_issue = match details.report.issue_id {
        Some(issue_id) => state
            .database
            .find_issue(issue_id)
            .await?
            .map(|issue| LinkedIssueView {
                id: issue.id,
                title: issue.title,
                github_number: issue.github_number,
                github_url: issue.github_url,
            }),
        None => None,
    };
    let title = title_for_report(&details);
    let issue_proposal = propose_issue(&details);
    let platform = field_string(&details.fields, "platform").unwrap_or_else(|| "Unknown".into());
    let architecture =
        field_string(&details.fields, "architecture").unwrap_or_else(|| "Unknown".into());

    let mut overview = vec![
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
    ];

    overview.push(OverviewField {
        label: "Submitted",
        value: details
            .report
            .created_at
            .format("%d %b %Y, %H:%M UTC")
            .to_string(),
        filter_url: Some(submitted_date_filter_url(
            details.report.created_at.date_naive(),
        )),
        full_width: true,
    });

    // Older clients only supplied the combined build envelope. Put that long
    // value on its own row, after the shorter submission details. Newer clients
    // expose its parts as regular fields below, so avoid repeating them here.
    if !details
        .fields
        .iter()
        .any(|field| field.key == "build_configuration")
    {
        let mut build = OverviewField::searchable(
            "Build",
            &details.report.build,
            "build",
            &details.report.build,
        );
        build.full_width = true;
        overview.push(build);
    }

    let report_view = ReportView {
        id: details.report.id,
        title,
        overview,
        is_assigned: details.report.issue_id.is_some(),
        is_confirmed: details.report.state == "confirmed",
        is_rejected: details.report.state == "rejected",
        has_submission_source: details.report.has_submission_source,
        submission_source_is_blocked: details.report.submission_source_is_blocked,
    };

    let mut known_fields = Vec::new();
    let mut unknown_fields = Vec::new();

    let signal = field_string(&details.fields, "signal");
    let process = field_string(&details.fields, "process");
    let report_kind = details.report.kind.clone();

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

        let is_stack = field.kind == "stack_trace"
            || field.current_kind.as_deref() == Some("stack_trace")
            || (key == "stack" && field.kind == "multiline");
        let stack = is_stack.then(|| parse_stack_trace(&value));
        let stack_signature = stack.as_ref().and_then(|parsed| {
            stack_fingerprint(
                &report_kind,
                process.as_deref(),
                signal.as_deref(),
                &parsed.frame_keys,
            )
            .map(|full| StackSignatureView {
                short: full[..12].to_owned(),
                full,
            })
        });
        let display_kind = if is_stack {
            "stack_trace".to_owned()
        } else {
            field.kind.clone()
        };
        let view = FieldView {
            label: field.current_label.clone().unwrap_or_else(|| key.clone()),
            is_multiline: is_stack || matches!(field.kind.as_str(), "multiline" | "stack_trace"),
            stack,
            stack_signature,
            filter_url: (!matches!(field.kind.as_str(), "multiline" | "stack_trace"))
                .then(|| field_filter_url(&key, &value)),
            kind: display_kind,
            key,
            value,
            known,
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
            media_type: attachment.media_type,
            size: format_attachment_size(attachment.size),
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
        asset_version: super::assets::asset_version(),
        report: report_view,
        linked_issue,
        potential_issues,
        issue_proposal,
        known_groups: group_fields(known_fields),
        unknown_groups: group_fields(unknown_fields),
        attachments,
        events,
    }))
}

fn group_fields(fields: Vec<FieldView>) -> Vec<FieldGroup> {
    let mut groups: Vec<FieldGroup> = Vec::new();

    for field in fields {
        let inline = !field.is_multiline;
        if inline
            && let Some(group) = groups.last_mut()
            && group.inline
        {
            group.fields.push(field);
            continue;
        }

        groups.push(FieldGroup {
            inline,
            fields: vec![field],
        });
    }

    groups
}

impl OverviewField {
    fn searchable(label: &'static str, value: &str, key: &str, search_value: &str) -> Self {
        Self {
            label,
            value: value.into(),
            filter_url: Some(field_filter_url(key, search_value)),
            full_width: false,
        }
    }
}

#[derive(Deserialize)]
pub struct IssueAssignmentForm {
    csrf: String,
    issue_selection: String,
}

pub async fn assign_to_issue(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
    Form(form): Form<IssueAssignmentForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    state.database.ensure_report_unassigned(report_id).await?;

    let issue_id = if let Some(value) = form.issue_selection.strip_prefix("issue:") {
        let issue_id = value
            .parse::<IssueId>()
            .map_err(|_| AppError::InvalidRequest("Select an issue"))?;
        state
            .database
            .assign_report_to_issue(report_id, issue_id, session.github_id)
            .await?;
        issue_id
    } else if let Some(value) = form.issue_selection.strip_prefix("github:") {
        let github_number = value
            .parse::<i64>()
            .map_err(|_| AppError::InvalidRequest("Select an issue"))?;
        if github_number < 1 {
            return Err(AppError::InvalidRequest("Select an issue"));
        }

        let configuration = state.database.configuration().await?;
        let access_token = session.github_access_token(&state)?;
        let github_issue = state
            .github
            .issue(
                &access_token,
                &configuration.github_repository,
                github_number,
            )
            .await?;
        state
            .database
            .assign_report_to_github_issue(
                &github_issue,
                report_id,
                &configuration.github_repository,
                session.github_id,
            )
            .await?
            .issue_id
    } else {
        return Err(AppError::InvalidRequest("Select an issue"));
    };

    tracing::info!(
        event = "report.update_issue",
        %report_id,
        %issue_id,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/issues/{issue_id}")))
}

#[derive(Deserialize)]
pub struct ReportActionForm {
    csrf: String,
}

#[derive(Deserialize)]
pub struct UnlinkReportForm {
    csrf: String,
    issue_id: IssueId,
}

pub async fn unlink_from_issue(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
    Form(form): Form<UnlinkReportForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    state
        .database
        .unlink_report_from_issue(form.issue_id, report_id, session.github_id)
        .await?;

    tracing::info!(
        event = "report.update_issue",
        %report_id,
        from = %form.issue_id,
        to = "none",
        actor = session.login,
    );
    Ok(Redirect::to(&format!("/reports/{report_id}")))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportWorkflowState {
    Triage,
    Confirmed,
}

impl ReportWorkflowState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Triage => "triage",
            Self::Confirmed => "confirmed",
        }
    }
}

#[derive(Deserialize)]
pub struct ReportStateForm {
    csrf: String,
    state: ReportWorkflowState,
}

pub async fn set_state(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
    Form(form): Form<ReportStateForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    let target = form.state.as_str();
    let previous = state
        .database
        .set_report_state(report_id, target, session.github_id)
        .await?;

    if let Some(previous) = previous {
        tracing::info!(
            event = "report.update_state",
            from = previous,
            to = target,
            %report_id,
            actor = session.login,
        );
    }
    Ok(Redirect::to(&format!("/reports/{report_id}")))
}

pub async fn reject(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
    Form(form): Form<ReportActionForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    let previous = state
        .database
        .set_report_state(report_id, "rejected", session.github_id)
        .await?;

    if let Some(previous) = previous {
        tracing::warn!(
            event = "report.update_state",
            %report_id,
            from = previous,
            to = "rejected",
            actor = session.login,
        );
    }
    Ok(Redirect::to(&format!("/reports/{report_id}")))
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
        event = "submission_source.update_state",
        %report_id,
        from = "allowed",
        to = "blocked",
        triage_reports_rejected = outcome.rejected_triage_reports,
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/reports/{report_id}")))
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
        event = "submission_source.update_state",
        %report_id,
        from = "blocked",
        to = "allowed",
        actor = session.login,
    );

    Ok(Redirect::to(&format!("/reports/{report_id}")))
}

pub async fn attachment(
    State(state): State<AdminState>,
    Path(attachment_id): Path<AttachmentId>,
    Query(display): Query<AttachmentDisplay>,
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
    let disposition = attachment_disposition(&attachment.name, display.inline);

    Ok((
        [
            ("content-type", content_type),
            ("content-disposition", disposition.as_str()),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct AttachmentDisplay {
    #[serde(default)]
    inline: bool,
}

fn attachment_disposition(name: &str, inline: bool) -> String {
    let fallback = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let encoded = name
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect::<String>();

    let kind = if inline { "inline" } else { "attachment" };
    format!("{kind}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

fn format_attachment_size(bytes: i64) -> String {
    let mut size = bytes as f64;
    let mut unit = "bytes";
    for next_unit in ["KiB", "MiB", "GiB", "TiB"] {
        if size < 1024.0 {
            break;
        }
        size /= 1024.0;
        unit = next_unit;
    }

    let precision = usize::from(unit != "bytes");
    format!("{size:.precision$} {unit}")
}

fn next_page_url(
    filters: &ReportFilters,
    last: Option<&crate::infrastructure::database::ReportSummary>,
) -> Option<String> {
    let last = last?;
    let mut url =
        reqwest::Url::parse("http://localhost/api/report-list").expect("static URL is valid");

    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("q", &filters.q)
            .append_pair("before", &last.created_at.to_rfc3339())
            .append_pair("before_id", &last.id.to_string());

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

    Some(format!(
        "/api/report-list?{}",
        url.query().unwrap_or_default()
    ))
}

fn report_state(state: &str) -> (&'static str, &'static str) {
    match state {
        "triage" => ("Needs triage", "triage"),
        "confirmed" => ("Confirmed", "confirmed"),
        "rejected" => ("Rejected", "rejected"),
        _ => ("Unknown state", "neutral"),
    }
}

fn default_report_search() -> String {
    "state:triage state:confirmed".into()
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
        .append_pair("q", &filter_expression(key, value));

    format!("/?{}", url.query().unwrap_or_default())
}

fn submitted_date_filter_url(date: NaiveDate) -> String {
    let mut url = reqwest::Url::parse("http://localhost/").expect("static URL is valid");
    url.query_pairs_mut()
        .append_pair("q", "")
        .append_pair("since", &date.to_string())
        .append_pair("until", &date.to_string());

    format!("/?{}", url.query().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::{attachment_disposition, format_attachment_size};

    #[test]
    fn attachment_sizes_use_binary_units() {
        assert_eq!(format_attachment_size(1023), "1023 bytes");
        assert_eq!(format_attachment_size(3756), "3.7 KiB");
        assert_eq!(format_attachment_size(1_048_576), "1.0 MiB");
    }

    #[test]
    fn attachment_names_are_safe_in_download_headers() {
        assert_eq!(
            attachment_disposition("crash notes-ä.txt", false),
            "attachment; filename=\"crash_notes-_.txt\"; filename*=UTF-8''crash%20notes-%C3%A4.txt"
        );
        assert_eq!(
            attachment_disposition("crash notes-ä.txt", true),
            "inline; filename=\"crash_notes-_.txt\"; filename*=UTF-8''crash%20notes-%C3%A4.txt"
        );
    }
}
