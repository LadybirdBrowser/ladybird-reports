use sqlx::{Postgres, QueryBuilder, Row};

use crate::{
    domain::{IssueId, IssueSearch, ReportId},
    error::{AppError, Result},
    infrastructure::{
        database::AdminDatabase,
        github::{GithubIssue, GithubIssueState},
    },
};

use super::{
    GithubIssueAssignment, GithubIssueLink, IssueDetails, IssueRecord, IssueSummary, ReportSummary,
};

impl AdminDatabase {
    pub async fn list_issues(&self, search: &IssueSearch) -> Result<Vec<IssueSummary>> {
        let mut sql = QueryBuilder::<Postgres>::new(
            "SELECT
                issues.id,
                issues.title,
                issues.state,
                issues.resolved_at,
                issues.github_number,
                issues.github_state,
                issues.created_at,
                count(reports.id) AS report_count
             FROM issues
             LEFT JOIN reports
                ON reports.issue_id = issues.id
                AND reports.storage_state = 'ready'
             WHERE issues.merged_into IS NULL",
        );

        if !search.states.is_empty() && !search.states.iter().any(|state| state == "all") {
            sql.push(" AND issues.state = ANY(")
                .push_bind(&search.states)
                .push(")");
        }
        if !search.github_numbers.is_empty() {
            sql.push(" AND (issues.github_number = ANY(")
                .push_bind(&search.github_numbers)
                .push(
                    ") OR EXISTS (
                    SELECT 1 FROM issue_github_aliases AS aliases
                    WHERE aliases.issue_id = issues.id
                        AND aliases.github_number = ANY(",
                )
                .push_bind(&search.github_numbers)
                .push(")))");
        }
        if !search.ids.is_empty() {
            sql.push(" AND (");
            for (index, id) in search.ids.iter().enumerate() {
                if index > 0 {
                    sql.push(" OR ");
                }
                sql.push("position(")
                    .push_bind(id)
                    .push(" in lower(issues.id::text)) > 0");
            }
            sql.push(")");
        }
        for term in &search.terms {
            let number = term.trim_start_matches('#');
            sql.push(" AND (position(lower(")
                .push_bind(term)
                .push(") in lower(issues.title)) > 0 OR position(")
                .push_bind(number)
                .push(" in issues.github_number::text) > 0 OR position(lower(")
                .push_bind(term)
                .push(") in lower(issues.id::text)) > 0)");
        }

        sql.push(" GROUP BY issues.id ORDER BY issues.updated_at DESC LIMIT 200");
        let rows = sql.build().fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|row| IssueSummary {
                id: row.get("id"),
                title: row.get("title"),
                state: row.get("state"),
                resolved_at: row.get("resolved_at"),
                github_number: row.get("github_number"),
                github_state: row.get("github_state"),
                report_count: row.get("report_count"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    pub async fn search_issues(&self, search: &str) -> Result<Vec<IssueSummary>> {
        let rows = sqlx::query(
            "SELECT
                issues.id,
                issues.title,
                issues.state,
                issues.resolved_at,
                issues.github_number,
                issues.github_state,
                issues.created_at,
                count(reports.id) AS report_count
             FROM issues
             LEFT JOIN reports
                ON reports.issue_id = issues.id
                AND reports.storage_state = 'ready'
             WHERE issues.merged_into IS NULL
                AND issues.state IN ('unresolved', 'needs_attention')
                AND issues.resolved_at IS NULL
                AND (
                    $1 = ''
                    OR position(lower($1) in lower(issues.title)) > 0
                    OR position(lower($1) in issues.id::text) > 0
                    OR issues.github_number::text = trim(leading '#' from $1)
                )
             GROUP BY issues.id
             ORDER BY
                CASE
                    WHEN $1 <> '' AND lower(issues.title) LIKE lower($1) || '%' THEN 0
                    ELSE 1
                END,
                issues.updated_at DESC
             LIMIT 20",
        )
        .bind(search)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| IssueSummary {
                id: row.get("id"),
                title: row.get("title"),
                state: row.get("state"),
                resolved_at: row.get("resolved_at"),
                github_number: row.get("github_number"),
                github_state: row.get("github_state"),
                report_count: row.get("report_count"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    pub async fn issues_linked_to_github_numbers(
        &self,
        repository: &str,
        github_numbers: &[i64],
    ) -> Result<Vec<GithubIssueLink>> {
        if github_numbers.is_empty() {
            return Ok(Vec::new());
        }

        let rows = sqlx::query(
            "WITH RECURSIVE linked AS (
                SELECT id, merged_into, github_number, 0 AS depth
                FROM issues
                WHERE lower(github_repository) = lower($1)
                    AND github_number = ANY($2)
                    AND state <> 'rejected'
                UNION ALL
                SELECT aliases.issue_id, issues.merged_into,
                       aliases.github_number, 0 AS depth
                FROM issue_github_aliases AS aliases
                JOIN issues ON issues.id = aliases.issue_id
                WHERE lower(aliases.github_repository) = lower($1)
                    AND aliases.github_number = ANY($2)
                    AND issues.state <> 'rejected'
                UNION ALL
                SELECT issues.id, issues.merged_into, linked.github_number,
                       linked.depth + 1
                FROM linked
                JOIN issues ON issues.id = linked.merged_into
                WHERE linked.depth < 16
                    AND issues.state <> 'rejected'
             )
             SELECT linked.id, linked.github_number, issues.title,
                    issues.github_state
             FROM linked
             JOIN issues ON issues.id = linked.id
             WHERE linked.merged_into IS NULL
                AND issues.state <> 'rejected'",
        )
        .bind(repository)
        .bind(github_numbers)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| GithubIssueLink {
                issue_id: row.get("id"),
                github_number: row.get("github_number"),
                title: row.get("title"),
                github_state: row.get("github_state"),
            })
            .collect())
    }

    pub async fn assign_report_to_github_issue(
        &self,
        issue: &GithubIssue,
        report_id: ReportId,
        repository: &str,
        actor: i64,
    ) -> Result<GithubIssueAssignment> {
        let description = issue.body.as_deref().unwrap_or_default();
        validate_issue_text(&issue.title, description)?;
        if issue.state != GithubIssueState::Open || !issue.belongs_to_repository(repository) {
            return Err(AppError::Conflict(
                "GitHub issue is not open in this repository",
            ));
        }

        self.sync_github_issue(repository, issue, Some(actor), "assignment")
            .await?;

        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        let existing_issue_id: Option<IssueId> = sqlx::query_scalar(
            "WITH RECURSIVE chain AS (
                SELECT id, merged_into, 0 AS depth
                FROM issues
                WHERE state <> 'rejected'
                    AND ((lower(github_repository) = lower($1)
                        AND github_number = $2)
                        OR github_issue_id = $3)
                UNION ALL
                SELECT aliases.issue_id, issues.merged_into, 0 AS depth
                FROM issue_github_aliases AS aliases
                JOIN issues ON issues.id = aliases.issue_id
                WHERE issues.state <> 'rejected'
                    AND ((lower(aliases.github_repository) = lower($1)
                        AND aliases.github_number = $2)
                        OR aliases.github_issue_id = $3)
                UNION ALL
                SELECT issues.id, issues.merged_into, chain.depth + 1
                FROM chain
                JOIN issues ON issues.id = chain.merged_into
                WHERE chain.depth < 16
                    AND issues.state <> 'rejected'
             )
             SELECT id FROM chain WHERE merged_into IS NULL
             ORDER BY depth DESC LIMIT 1",
        )
        .bind(repository)
        .bind(issue.number)
        .bind(issue.id)
        .fetch_optional(&mut *transaction)
        .await?;

        let (issue_id, created) = if let Some(issue_id) = existing_issue_id {
            let active: bool = sqlx::query_scalar(
                "SELECT resolved_at IS NULL AND github_state IN ('open', 'unknown')
                 FROM issues WHERE id = $1 AND state <> 'rejected'",
            )
            .bind(issue_id)
            .fetch_one(&mut *transaction)
            .await?;
            if !active {
                return Err(AppError::Conflict("Tracked issue is not open"));
            }
            (issue_id, false)
        } else {
            let issue_id = IssueId::new();
            sqlx::query(
                "INSERT INTO issues (
                    id,
                    title,
                    description,
                    github_number,
                    github_url,
                    github_repository,
                    github_issue_id,
                    github_state,
                    github_checked_at,
                    github_updated_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'open', now(), $8)",
            )
            .bind(issue_id)
            .bind(issue.title.trim())
            .bind(description)
            .bind(issue.number)
            .bind(&issue.html_url)
            .bind(repository)
            .bind(issue.id)
            .bind(issue.updated_at)
            .execute(&mut *transaction)
            .await?;

            sqlx::query(
                "INSERT INTO audit_events (actor, action, entity_id, details)
                 VALUES ($1, 'issue.create', $2, $3)",
            )
            .bind(actor)
            .bind(issue_id.0)
            .bind(serde_json::json!({
                "github_number": issue.number,
                "github_url": issue.html_url,
                "report_id": report_id,
            }))
            .execute(&mut *transaction)
            .await?;

            (issue_id, true)
        };

        let previous: (Option<IssueId>, String) = sqlx::query_as(
            "SELECT issue_id, state
             FROM reports
             WHERE id = $1
                AND storage_state = 'ready'
             FOR UPDATE",
        )
        .bind(report_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("Report not found"))?;

        if previous.0 == Some(issue_id) && previous.1 == "confirmed" {
            transaction.commit().await?;
            return Ok(GithubIssueAssignment { issue_id, created });
        }
        if previous.0.is_some() && previous.0 != Some(issue_id) {
            return Err(AppError::Conflict(
                "Unlink the report before adding it to another issue",
            ));
        }

        let updated = sqlx::query(
            "UPDATE reports
             SET issue_id = $2, state = 'confirmed', updated_at = now()
             WHERE id = $1
                AND storage_state = 'ready'",
        )
        .bind(report_id)
        .bind(issue_id)
        .execute(&mut *transaction)
        .await?;

        if updated.rows_affected() != 1 {
            return Err(AppError::NotFound("Report not found"));
        }

        if previous.0 != Some(issue_id) {
            sqlx::query(
                "INSERT INTO audit_events (actor, action, entity_id, details)
                 VALUES ($1, 'report.update_issue', $2, $3)",
            )
            .bind(actor)
            .bind(report_id.0)
            .bind(serde_json::json!({
                "from": previous.0,
                "to": issue_id,
            }))
            .execute(&mut *transaction)
            .await?;
        }

        if previous.1 != "confirmed" {
            self.audit_report_state_change(
                &mut transaction,
                report_id,
                actor,
                &previous.1,
                "confirmed",
            )
            .await?;
        }

        transaction.commit().await?;

        Ok(GithubIssueAssignment { issue_id, created })
    }

    pub async fn issue_details(&self, issue_id: IssueId) -> Result<Option<IssueDetails>> {
        let issue = self.find_issue(issue_id).await?;

        let Some(issue) = issue else {
            return Ok(None);
        };

        let report_rows = sqlx::query(
            "SELECT id, kind, client_version, state, created_at
             FROM reports
             WHERE issue_id = $1
                AND storage_state = 'ready'
             ORDER BY created_at DESC",
        )
        .bind(issue_id)
        .fetch_all(&self.pool)
        .await?;

        let mut reports = report_rows
            .into_iter()
            .map(|row| ReportSummary {
                id: row.get("id"),
                title: String::new(),
                kind: row.get("kind"),
                client_version: row.get("client_version"),
                platform: None,
                architecture: None,
                state: row.get("state"),
                created_at: row.get("created_at"),
            })
            .collect::<Vec<_>>();
        self.populate_report_titles(&mut reports).await?;

        let events = self.audit_events(issue_id.0).await?;

        Ok(Some(IssueDetails {
            issue,
            reports,
            events,
        }))
    }

    pub async fn find_issue(&self, issue_id: IssueId) -> Result<Option<IssueRecord>> {
        let row = sqlx::query(
            "SELECT
                issues.id,
                issues.title,
                issues.description,
                issues.state,
                issues.resolved_at,
                issues.merged_into,
                issues.github_number,
                issues.github_repository,
                issues.github_issue_id,
                issues.github_state,
                issues.github_url,
                issues.github_reports_field_id,
                issues.github_reports_link_url,
                issues.github_checked_at,
                issues.created_at,
                issues.updated_at
             FROM issues
             WHERE issues.id = $1",
        )
        .bind(issue_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| IssueRecord {
            id: row.get("id"),
            title: row.get("title"),
            description: row.get("description"),
            state: row.get("state"),
            resolved_at: row.get("resolved_at"),
            merged_into: row.get("merged_into"),
            github_number: row.get("github_number"),
            github_repository: row.get("github_repository"),
            github_issue_id: row.get("github_issue_id"),
            github_state: row.get("github_state"),
            github_url: row.get("github_url"),
            github_reports_field_id: row.get("github_reports_field_id"),
            github_reports_link_url: row.get("github_reports_link_url"),
            github_checked_at: row.get("github_checked_at"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        }))
    }

    pub async fn unlink_report_from_issue(
        &self,
        issue_id: IssueId,
        report_id: ReportId,
        actor: i64,
    ) -> Result<()> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        let issue_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM issues WHERE id = $1
             )",
        )
        .bind(issue_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !issue_exists {
            return Err(AppError::NotFound("Issue not found"));
        }

        let previous_state: String = sqlx::query_scalar(
            "SELECT state FROM reports
             WHERE id = $1 AND issue_id = $2 AND storage_state = 'ready'
             FOR UPDATE",
        )
        .bind(report_id)
        .bind(issue_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("Assigned report not found"))?;

        sqlx::query(
            "UPDATE reports
             SET issue_id = NULL, state = 'triage', updated_at = now()
             WHERE id = $1 AND issue_id = $2
                AND storage_state = 'ready'",
        )
        .bind(report_id)
        .bind(issue_id)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id, details)
             VALUES ($1, 'report.update_issue', $2,
                jsonb_build_object('from', $3::uuid, 'to', NULL))",
        )
        .bind(actor)
        .bind(report_id.0)
        .bind(issue_id)
        .execute(&mut *transaction)
        .await?;

        if previous_state != "triage" {
            self.audit_report_state_change(
                &mut transaction,
                report_id,
                actor,
                &previous_state,
                "triage",
            )
            .await?;
        }

        transaction.commit().await?;
        Ok(())
    }

    pub async fn reject_issue(
        &self,
        issue_id: IssueId,
        actor: i64,
        reject_reports: bool,
    ) -> Result<u64> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        let previous_state: String = sqlx::query_scalar(
            "SELECT state FROM issues WHERE id = $1 AND state <> 'rejected' FOR UPDATE",
        )
        .bind(issue_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("Issue not found"))?;

        let result = sqlx::query(
            "UPDATE issues
             SET state = 'rejected', updated_at = now()
             WHERE id = $1 AND state <> 'rejected'",
        )
        .bind(issue_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(AppError::NotFound("Issue not found"));
        }

        // Merged source records point at this issue, so reject them as well.
        let merged_sources_rejected = sqlx::query(
            "WITH RECURSIVE sources AS (
                SELECT id FROM issues WHERE merged_into = $1
                UNION ALL
                SELECT issues.id FROM issues
                JOIN sources ON issues.merged_into = sources.id
             )
             UPDATE issues
             SET state = 'rejected', updated_at = now()
             WHERE id IN (SELECT id FROM sources) AND state <> 'rejected'",
        )
        .bind(issue_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        let reports_updated: i64 = sqlx::query_scalar(
            "WITH previous AS MATERIALIZED (
                SELECT id, state FROM reports WHERE issue_id = $1 FOR UPDATE
             ),
             updated AS (
                UPDATE reports
                SET issue_id = CASE WHEN $3 THEN reports.issue_id ELSE NULL END,
                    state = CASE WHEN $3 THEN 'rejected' ELSE 'triage' END,
                    updated_at = now()
                FROM previous
                WHERE reports.id = previous.id
                RETURNING reports.id, previous.state AS previous_state
             ),
             issue_events AS (
                INSERT INTO audit_events (actor, action, entity_id, details)
                SELECT $2, 'report.update_issue', id,
                       jsonb_build_object('from', $1::uuid, 'to', NULL)
                FROM updated WHERE NOT $3
                RETURNING 1
             ),
             state_events AS (
                INSERT INTO audit_events (actor, action, entity_id, details)
                SELECT $2, 'report.update_state', id,
                       jsonb_build_object(
                           'from', previous_state,
                           'to', CASE WHEN $3 THEN 'rejected' ELSE 'triage' END
                       )
                FROM updated
                WHERE previous_state <> CASE WHEN $3 THEN 'rejected' ELSE 'triage' END
                RETURNING 1
             )
             SELECT count(*) FROM updated",
        )
        .bind(issue_id)
        .bind(actor)
        .bind(reject_reports)
        .fetch_one(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id, details)
             VALUES ($1, 'issue.update_state', $2, $3)",
        )
        .bind(actor)
        .bind(issue_id.0)
        .bind(serde_json::json!({
            "from": previous_state,
            "to": "rejected",
            "report_action": if reject_reports { "reject" } else { "unlink" },
            "reports_updated": reports_updated,
            "merged_sources_rejected": merged_sources_rejected,
        }))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(reports_updated as u64)
    }

    pub async fn merge_issue(
        &self,
        source: IssueId,
        destination: IssueId,
        actor: i64,
    ) -> Result<()> {
        if source == destination {
            return Err(AppError::InvalidRequest(
                "An issue cannot be merged into itself",
            ));
        }

        let mut transaction = self.pool.begin().await?;

        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        let destination_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM issues
                WHERE id = $1 AND merged_into IS NULL
                    AND state <> 'rejected'
                    AND resolved_at IS NULL AND github_state = 'open'
            )",
        )
        .bind(destination)
        .fetch_one(&mut *transaction)
        .await?;

        if !destination_exists {
            return Err(AppError::InvalidRequest("Destination issue is not active"));
        }

        let source_update = sqlx::query(
            "UPDATE issues
             SET merged_into = $2, updated_at = now()
             WHERE id = $1 AND merged_into IS NULL
                AND state <> 'rejected'",
        )
        .bind(source)
        .bind(destination)
        .execute(&mut *transaction)
        .await?;

        if source_update.rows_affected() != 1 {
            return Err(AppError::NotFound("Source issue not found"));
        }

        let moved_reports = sqlx::query(
            "WITH moved AS (
                UPDATE reports
                SET issue_id = $2, updated_at = now()
                WHERE issue_id = $1
                RETURNING id
             )
             INSERT INTO audit_events (actor, action, entity_id, details)
             SELECT
                $3,
                'report.update_issue',
                moved.id,
                jsonb_build_object('from', $1::uuid, 'to', $2::uuid)
             FROM moved",
        )
        .bind(source)
        .bind(destination)
        .bind(actor)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        sqlx::query(
            "INSERT INTO audit_events (actor, action, entity_id, details)
             VALUES ($1, 'issue.merge', $2, $3)",
        )
        .bind(actor)
        .bind(source.0)
        .bind(serde_json::json!({
            "into": destination,
            "reports_moved": moved_reports,
        }))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }
}

pub(crate) fn validate_issue_text(title: &str, description: &str) -> Result<()> {
    if title.trim().is_empty() || title.chars().count() > 256 {
        return Err(AppError::InvalidRequest(
            "Issue title must be 1 to 256 characters",
        ));
    }

    if description.chars().count() > 256 * 1024 {
        return Err(AppError::InvalidRequest("Issue description is too large"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_issue_text;

    #[test]
    fn github_issue_text_limits_count_characters() {
        assert!(validate_issue_text(&"界".repeat(256), "").is_ok());
        assert!(validate_issue_text(&"界".repeat(257), "").is_err());
    }
}
