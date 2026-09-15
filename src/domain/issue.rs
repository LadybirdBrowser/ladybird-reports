use chrono::{DateTime, Utc};

use crate::domain::IssueId;

#[derive(Clone, Debug)]
pub struct Issue {
    pub id: IssueId,
    pub title: String,
    pub description: String,
    pub resolved_at: Option<DateTime<Utc>>,
    pub merged_into: Option<IssueId>,
    pub github_number: Option<i64>,
    pub github_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
