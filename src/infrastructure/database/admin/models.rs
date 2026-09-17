use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde_json::Value;

use crate::domain::{
    AttachmentId, DiscordDeliveryLeaseId, IssueId, ReportId, ReportSearch, SubmissionId, UploadId,
};

#[derive(Clone, Debug)]
pub struct SessionRecord {
    pub github_id: i64,
    pub login: String,
    pub csrf_token: String,
    pub token_hash: String,
    pub encrypted_access_token: String,
    pub membership_verified_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

pub struct NewSession<'a> {
    pub github_id: i64,
    pub login: &'a str,
    pub authorized_team: &'a str,
    pub token_hash: &'a str,
    pub encrypted_access_token: &'a str,
    pub csrf_token: &'a str,
    pub lifetime: Duration,
}

#[derive(Clone, Debug, Default)]
pub struct ReportQuery {
    pub search: ReportSearch,
    pub issue_id: Option<IssueId>,
    pub since: Option<NaiveDate>,
    pub until: Option<NaiveDate>,
    pub before: Option<DateTime<Utc>>,
    pub before_id: Option<ReportId>,
}

#[derive(Clone, Debug)]
pub struct ReportSummary {
    pub id: ReportId,
    pub title: String,
    pub kind: String,
    pub client_version: String,
    pub build: String,
    pub platform: Option<String>,
    pub architecture: Option<String>,
    pub issue_id: Option<IssueId>,
    pub state: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct GithubIssueLink {
    pub issue_id: IssueId,
    pub github_number: i64,
    pub title: String,
    pub github_state: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GithubIssueAssignment {
    pub issue_id: IssueId,
    pub created: bool,
}

#[derive(Clone, Debug)]
pub struct PendingDiscordNotification {
    pub report_id: ReportId,
    pub lease_id: DiscordDeliveryLeaseId,
    pub kind: String,
    pub client_version: String,
    pub build: String,
    pub fields: Value,
    pub created_at: DateTime<Utc>,
    pub attempt_count: u32,
}

#[derive(Clone, Debug)]
pub struct ReportSearchResult {
    pub id: ReportId,
    pub title: String,
    pub kind: String,
    pub client_version: String,
    pub build: String,
    pub platform: Option<String>,
    pub architecture: Option<String>,
    pub issue_title: Option<String>,
    pub state: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ReportRecord {
    pub id: ReportId,
    pub submission_id: SubmissionId,
    pub manifest_digest: String,
    pub kind: String,
    pub client_version: String,
    pub build: String,
    pub issue_id: Option<IssueId>,
    pub state: String,
    pub source_ip: Option<String>,
    pub has_submission_source: bool,
    pub submission_source_is_blocked: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct StoredDiagnosticField {
    pub key: String,
    pub kind: String,
    pub value: Value,
    pub recognized_at_submission: bool,
    pub current_label: Option<String>,
    pub current_kind: Option<String>,
    pub current_position: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct StoredAttachment {
    pub id: AttachmentId,
    pub name: String,
    pub media_type: String,
    pub size: i64,
    pub sha256: String,
    pub storage_key: String,
}

#[derive(Clone, Debug)]
pub struct ReportDetails {
    pub report: ReportRecord,
    pub fields: Vec<StoredDiagnosticField>,
    pub attachments: Vec<StoredAttachment>,
    pub events: Vec<AuditEvent>,
}

#[derive(Clone, Debug)]
pub struct SimilarReport {
    pub report_id: ReportId,
    pub issue_id: Option<IssueId>,
    pub issue_title: Option<String>,
    pub client_version: String,
    pub created_at: DateTime<Utc>,
    pub exact: bool,
    pub matching_frames: usize,
    pub score: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockReportSourceOutcome {
    pub rejected_triage_reports: u64,
}

#[derive(Clone, Debug)]
pub struct IssueSummary {
    pub id: IssueId,
    pub title: String,
    pub resolved_at: Option<DateTime<Utc>>,
    pub github_number: i64,
    pub github_state: String,
    pub report_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct IssueRecord {
    pub id: IssueId,
    pub title: String,
    pub description: String,
    pub resolved_at: Option<DateTime<Utc>>,
    pub merged_into: Option<IssueId>,
    pub github_number: i64,
    pub github_repository: String,
    pub github_issue_id: Option<i64>,
    pub github_state: String,
    pub github_url: String,
    pub github_reports_field_id: Option<i64>,
    pub github_reports_link_url: Option<String>,
    pub github_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct IssueDetails {
    pub issue: IssueRecord,
    pub reports: Vec<ReportSummary>,
    pub events: Vec<AuditEvent>,
}

#[derive(Clone, Debug)]
pub struct AuditEvent {
    pub action: String,
    pub actor_login: Option<String>,
    pub entity_id: Option<uuid::Uuid>,
    pub details: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ConfigurationRecord {
    pub value: Value,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct FieldDefinitionRecord {
    pub key: String,
    pub label: String,
    pub kind: String,
    pub position: i32,
}

#[derive(Clone, Debug)]
pub struct PendingStorageRecord {
    pub report_id: ReportId,
    pub staging_id: UploadId,
}
