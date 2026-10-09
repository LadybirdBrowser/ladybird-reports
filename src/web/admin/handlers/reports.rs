use askama::Template;
use axum::{
    Extension, Form, Json,
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    application::{IssueProposal, propose_issue},
    domain::{
        AttachmentId, FieldKind, GithubLinkState, IssueId, ParsedStackTrace, ReportId, ReportKind,
        ReportSearch, ReportState, filter_expression, is_commit_id, parse_stack_trace,
        stack_fingerprint,
    },
    error::{AppError, Result},
    infrastructure::database::{
        IssueRecord, PotentialIssueMatch, REPORT_PAGE_SIZE, ReportQuery, ReportSummary,
        SEARCH_VALUE_LIMIT,
    },
};

use super::super::{
    AdminState, TemplateResponse,
    authentication::Navigation,
    session::{CsrfForm, Session},
    templates::{HistoryEvent, display_timestamp},
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
    pub(super) id: ReportId,
    pub(super) title: String,
    pub(super) metadata: String,
    state_label: &'static str,
    state_tone: &'static str,
    pub(super) received_at: String,
}

impl From<ReportSummary> for ReportRow {
    fn from(report: ReportSummary) -> Self {
        let (state_label, state_tone) = report_state(report.state);

        Self {
            metadata: report_list_metadata(&report),
            received_at: display_timestamp(report.created_at),
            id: report.id,
            title: report.title,
            state_label,
            state_tone,
        }
    }
}

#[derive(Deserialize)]
pub struct ReportSearchQuery {
    #[serde(default)]
    pub(super) query: String,
}

#[derive(Deserialize)]
pub struct CompletionQuery {
    #[serde(default)]
    pub(super) token: String,
}

#[derive(Serialize)]
pub struct CompletionResponse {
    pub(super) results: Vec<Completion>,
}

#[derive(Serialize)]
pub struct Completion {
    pub(super) replacement: String,
    pub(super) label: String,
    pub(super) description: String,
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
    linked_issue: Option<IssueRecord>,
    potential_issues: Vec<PotentialIssueMatch>,
    issue_proposal: IssueProposal,
    known_groups: Vec<FieldGroup>,
    unknown_groups: Vec<FieldGroup>,
    attachments: Vec<AttachmentView>,
    events: Vec<HistoryEvent>,
    url_dialog_field: CopyTarget,
}

/// What the copy button in the untrusted URL dialog says it copies.
pub struct CopyTarget {
    label: &'static str,
}

pub struct ReportView {
    id: ReportId,
    title: String,
    state: ReportState,
    overview: Vec<OverviewField>,
    is_assigned: bool,
    has_submission_source: bool,
    submission_source_is_blocked: bool,
}

pub struct OverviewField {
    label: &'static str,
    value: String,
    filter_url: Option<String>,
    full_width: bool,
    link: FieldLink,
}

/// How a field value is made clickable.
pub enum FieldLink {
    None,
    /// An address from the client. Never linked directly: a dialog warns first.
    UntrustedUrl,
    /// The commit in the upstream repository, built here from a validated ID.
    Commit(String),
}

pub struct FieldView {
    key: String,
    label: String,
    kind: FieldKind,
    value: String,
    is_multiline: bool,
    stack: Option<ParsedStackTrace>,
    stack_signature: Option<StackSignatureView>,
    filter_url: Option<String>,
    link: FieldLink,
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
    let has_more = reports.len() > REPORT_PAGE_SIZE;
    reports.truncate(REPORT_PAGE_SIZE);
    let next_page = has_more
        .then(|| next_page_url(filters, reports.last()))
        .flatten();
    let reports = reports.into_iter().map(ReportRow::from).collect();

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
            let (state_label, badge_tone) = report_state(report.state);
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
                footnote: format!("Received {}", display_timestamp(report.created_at)),
                group: None,
            }
        })
        .collect();

    Ok(Json(EntitySearchResponse { results }))
}

pub async fn search_completions(
    State(state): State<AdminState>,
    Query(parameters): Query<CompletionQuery>,
) -> Result<Json<CompletionResponse>> {
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
            .map(|(key, description)| Completion {
                replacement: format!("{key}:"),
                label: format!("{key}:"),
                description: description.to_owned(),
            })
            .collect();

        return Ok(Json(CompletionResponse { results }));
    };

    let key = key.to_ascii_lowercase();
    let prefix = value_prefix.trim_matches('"').to_ascii_lowercase();
    let values = match key.as_str() {
        "state" => ReportState::ALL
            .iter()
            .map(|state| state.as_str().to_owned())
            .collect(),
        "kind" => ReportKind::ALL
            .iter()
            .map(|kind| kind.as_str().to_owned())
            .collect(),
        "id" | "report" => Vec::new(),
        _ => {
            let values = state.database.report_search_values(&key).await?;
            if values.len() > SEARCH_VALUE_LIMIT {
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
        .map(|value| Completion {
            replacement: filter_expression(&key, &value),
            label: value,
            description: format!("Value for {key}"),
        })
        .collect();

    Ok(Json(CompletionResponse { results }))
}

fn report_kind_label(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Crash => "Crash report",
        ReportKind::WebCompat => "Web compatibility report",
    }
}

pub(super) fn report_list_metadata(report: &ReportSummary) -> String {
    let metadata = report_metadata(
        report.platform.as_deref(),
        report.architecture.as_deref(),
        &report.client_version,
    );
    let url = report
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty());

    match url {
        Some(url) if metadata.is_empty() => url.to_owned(),
        Some(url) => format!("{metadata} · {url}"),
        None => metadata,
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
        .ok_or_else(|| AppError::NotFound("Report not found"))?;
    let potential_issues = if details.report.issue_id.is_none() {
        state.database.potential_issue_matches(report_id).await?
    } else {
        Vec::new()
    };
    let linked_issue = match details.report.issue_id {
        Some(issue_id) => state.database.find_issue(issue_id).await?,
        None => None,
    };
    let issue_proposal = propose_issue(&details);
    let platform = field_string(&details.fields, "platform").unwrap_or_else(|| "Unknown".into());
    let architecture =
        field_string(&details.fields, "architecture").unwrap_or_else(|| "Unknown".into());
    let github_repository = state
        .database
        .configuration()
        .await?
        .github_repository
        .clone();
    let page_url = field_string(&details.fields, "url").filter(|url| !url.trim().is_empty());

    let mut overview = vec![
        OverviewField::searchable(
            "Report type",
            report_kind_label(details.report.kind),
            "kind",
            details.report.kind.as_str(),
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

    // Where the report came from belongs with the other facts at the top, but
    // only when the client sent one.
    if let Some(page_url) = &page_url {
        let mut field = OverviewField::searchable("Page URL", page_url, "url", page_url);
        field.full_width = true;
        field.link = untrusted_url_link(page_url);
        overview.push(field);
    }

    overview.push(OverviewField {
        label: "Submitted",
        value: display_timestamp(details.report.created_at),
        filter_url: Some(submitted_date_filter_url(
            details.report.created_at.date_naive(),
        )),
        full_width: true,
        link: FieldLink::None,
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
        title: issue_proposal.title.clone(),
        state: details.report.state,
        overview,
        is_assigned: details.report.issue_id.is_some(),
        has_submission_source: details.report.has_submission_source,
        submission_source_is_blocked: details.report.submission_source_is_blocked,
    };

    let mut known_fields = Vec::new();
    let mut unknown_fields = Vec::new();

    let signal = field_string(&details.fields, "signal");
    let process = field_string(&details.fields, "process");
    let failure_reason = field_string(&details.fields, "failure_reason");
    let report_kind = details.report.kind;

    for field in details.fields {
        let value = field
            .value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| field.value.to_string());
        let known = field.current_label.is_some();
        let is_stack = field.is_stack_trace();
        let key = field.key;

        // Shown in the overview instead, as long as there is something to show.
        if matches!(key.as_str(), "platform" | "architecture")
            || (key == "url" && page_url.is_some())
        {
            continue;
        }

        let stack = is_stack.then(|| parse_stack_trace(&value));
        let stack_signature = stack.as_ref().and_then(|parsed| {
            stack_fingerprint(
                report_kind,
                process.as_deref(),
                signal.as_deref(),
                failure_reason.as_deref(),
                parsed,
            )
            .map(|full| StackSignatureView {
                short: full[..12].to_owned(),
                full,
            })
        });
        let linked_kind = match field.current_kind {
            Some(kind @ (FieldKind::Url | FieldKind::CommitId)) => kind,
            _ => field.kind,
        };
        let display_kind = if is_stack {
            FieldKind::StackTrace
        } else {
            linked_kind
        };
        let link = match linked_kind {
            FieldKind::Url => untrusted_url_link(&value),
            FieldKind::CommitId if is_commit_id(&value) => FieldLink::Commit(format!(
                "https://github.com/{github_repository}/commit/{}",
                value.to_ascii_lowercase()
            )),
            _ => FieldLink::None,
        };
        let holds_text_block = matches!(field.kind, FieldKind::Multiline | FieldKind::StackTrace);
        let view = FieldView {
            label: field.current_label.clone().unwrap_or_else(|| key.clone()),
            is_multiline: is_stack || holds_text_block,
            stack,
            stack_signature,
            filter_url: (!holds_text_block).then(|| field_filter_url(&key, &value)),
            link,
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

    let events = details.events.into_iter().map(HistoryEvent::from).collect();

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
        url_dialog_field: CopyTarget { label: "URL" },
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
            link: FieldLink::None,
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
    Ok(report_redirect(report_id))
}

/// The states a maintainer can set directly. Rejecting has its own action.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportWorkflowState {
    Triage,
    Confirmed,
}

impl From<ReportWorkflowState> for ReportState {
    fn from(state: ReportWorkflowState) -> Self {
        match state {
            ReportWorkflowState::Triage => Self::Triage,
            ReportWorkflowState::Confirmed => Self::Confirmed,
        }
    }
}

fn report_redirect(report_id: ReportId) -> Redirect {
    Redirect::to(&format!("/reports/{report_id}"))
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
    let target = ReportState::from(form.state);
    let previous = state
        .database
        .set_report_state(report_id, target, session.github_id)
        .await?;

    if let Some(previous) = previous {
        tracing::info!(
            event = "report.update_state",
            from = %previous,
            to = %target,
            %report_id,
            actor = session.login,
        );
    }
    Ok(report_redirect(report_id))
}

pub async fn reject(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Path(report_id): Path<ReportId>,
    Form(form): Form<CsrfForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;
    let previous = state
        .database
        .set_report_state(report_id, ReportState::Rejected, session.github_id)
        .await?;

    if let Some(previous) = previous {
        tracing::warn!(
            event = "report.update_state",
            %report_id,
            from = %previous,
            to = %ReportState::Rejected,
            actor = session.login,
        );
    }
    Ok(report_redirect(report_id))
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

    Ok(report_redirect(report_id))
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

    Ok(report_redirect(report_id))
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

/// The label shown for a state and the badge tone, which is named after it.
fn report_state(state: ReportState) -> (&'static str, &'static str) {
    let label = match state {
        ReportState::Triage => "Needs triage",
        ReportState::Confirmed => "Confirmed",
        ReportState::Rejected => "Rejected",
    };

    (label, state.as_str())
}

fn default_report_search() -> String {
    "state:triage state:confirmed".into()
}

/// Only web addresses are offered for opening; anything else stays plain text.
fn untrusted_url_link(value: &str) -> FieldLink {
    match reqwest::Url::parse(value.trim()) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => FieldLink::UntrustedUrl,
        _ => FieldLink::None,
    }
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
