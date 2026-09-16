use std::collections::HashSet;

use sqlx::Row;

use crate::{
    domain::{IssueId, ReportId, STACK_SIGNATURE_VERSION, parse_stack_trace, stack_fingerprint},
    error::Result,
    infrastructure::database::AdminDatabase,
};

use super::SimilarReport;

impl AdminDatabase {
    /// Rebuild a bounded batch. Old signatures are replaced when the algorithm changes.
    pub async fn index_pending_stack_traces(&self) -> Result<usize> {
        let pending = self.stack_traces_to_index(None, 50).await?;
        let count = pending.len();
        self.store_stack_signatures(pending).await?;
        Ok(count)
    }

    pub async fn index_report_stack_traces(&self, report_id: ReportId) -> Result<()> {
        let pending = self.stack_traces_to_index(Some(report_id), 64).await?;
        self.store_stack_signatures(pending).await
    }

    async fn stack_traces_to_index(
        &self,
        report_id: Option<ReportId>,
        limit: i64,
    ) -> Result<Vec<PendingStackTrace>> {
        let rows = sqlx::query(
            "SELECT reports.id AS report_id, reports.kind, fields.key,
                    fields.value #>> '{}' AS text,
                    signal.value #>> '{}' AS signal,
                    process.value #>> '{}' AS process
             FROM report_fields AS fields
             JOIN reports ON reports.id = fields.report_id
             LEFT JOIN field_definitions AS definitions ON definitions.key = fields.key
             LEFT JOIN report_fields AS signal
                ON signal.report_id = reports.id AND signal.key = 'signal'
             LEFT JOIN report_fields AS process
                ON process.report_id = reports.id AND process.key = 'process'
             LEFT JOIN report_stack_signatures AS signatures
                ON signatures.report_id = fields.report_id
                AND signatures.field_key = fields.key
             WHERE reports.storage_state = 'ready'
                AND reports.deleted_at IS NULL
                AND ($1::uuid IS NULL OR reports.id = $1)
                AND (fields.kind = 'stack_trace'
                    OR (fields.kind = 'multiline'
                        AND (fields.key = 'stack' OR definitions.kind = 'stack_trace')))
                AND (signatures.report_id IS NULL
                    OR signatures.algorithm_version <> $2)
             ORDER BY reports.created_at, reports.id, fields.key
             LIMIT $3",
        )
        .bind(report_id)
        .bind(STACK_SIGNATURE_VERSION)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| PendingStackTrace {
                report_id: row.get("report_id"),
                kind: row.get("kind"),
                key: row.get("key"),
                text: row.get("text"),
                signal: row.get("signal"),
                process: row.get("process"),
            })
            .collect())
    }

    async fn store_stack_signatures(&self, pending: Vec<PendingStackTrace>) -> Result<()> {
        for trace in pending {
            let parsed = parse_stack_trace(&trace.text);
            let fingerprint = stack_fingerprint(
                &trace.kind,
                trace.process.as_deref(),
                trace.signal.as_deref(),
                &parsed.frame_keys,
            );
            let status = if fingerprint.is_some() {
                "parsed"
            } else {
                "insufficient"
            };

            sqlx::query(
                "INSERT INTO report_stack_signatures
                    (report_id, field_key, algorithm_version, status,
                     fingerprint, frame_keys)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (report_id, field_key) DO UPDATE SET
                    algorithm_version = excluded.algorithm_version,
                    status = excluded.status,
                    fingerprint = excluded.fingerprint,
                    frame_keys = excluded.frame_keys,
                    indexed_at = now()",
            )
            .bind(trace.report_id)
            .bind(trace.key)
            .bind(STACK_SIGNATURE_VERSION)
            .bind(status)
            .bind(fingerprint)
            .bind(parsed.frame_keys)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    pub async fn similar_reports(&self, report_id: ReportId) -> Result<Vec<SimilarReport>> {
        let rows = sqlx::query(
            "WITH source AS (
                SELECT fingerprint, frame_keys
                FROM report_stack_signatures
                WHERE report_id = $1 AND algorithm_version = $2
                    AND cardinality(frame_keys) >= 1
                ORDER BY field_key = 'stack' DESC, field_key
                LIMIT 1
             )
             SELECT candidates.report_id, candidates.fingerprint,
                    candidates.frame_keys, source.fingerprint AS source_fingerprint,
                    source.frame_keys AS source_frames,
                    reports.issue_id, issues.title AS issue_title,
                    reports.client_version, reports.created_at
             FROM source
             JOIN report_stack_signatures AS candidates
                ON candidates.report_id <> $1
                AND candidates.algorithm_version = $2
                AND (candidates.fingerprint = source.fingerprint
                    OR candidates.frame_keys && source.frame_keys)
             JOIN reports ON reports.id = candidates.report_id
             JOIN reports AS original ON original.id = $1
             LEFT JOIN issues ON issues.id = reports.issue_id
             WHERE reports.kind = original.kind
                AND reports.storage_state = 'ready'
                AND reports.hidden_at IS NULL
                AND reports.deleted_at IS NULL
                AND (reports.issue_id IS NULL
                    OR (issues.hidden_at IS NULL
                        AND issues.merged_into IS NULL
                        AND issues.resolved_at IS NULL
                        AND issues.github_state IN ('open', 'unknown')))
             ORDER BY candidates.fingerprint = source.fingerprint DESC,
                      reports.created_at DESC
             LIMIT 200",
        )
        .bind(report_id)
        .bind(STACK_SIGNATURE_VERSION)
        .fetch_all(&self.pool)
        .await?;

        let mut matches = Vec::new();
        for row in rows {
            let source_frames: Vec<String> = row.get("source_frames");
            let candidate_frames: Vec<String> = row.get("frame_keys");
            let source_fingerprint: Option<String> = row.get("source_fingerprint");
            let candidate_fingerprint: Option<String> = row.get("fingerprint");
            let exact = source_fingerprint.is_some() && source_fingerprint == candidate_fingerprint;
            let (matching_frames, score) = compare_frames(&source_frames, &candidate_frames);
            if !exact && matching_frames < 2 {
                continue;
            }

            matches.push(SimilarReport {
                report_id: row.get("report_id"),
                issue_id: row.get("issue_id"),
                issue_title: row.get("issue_title"),
                client_version: row.get("client_version"),
                created_at: row.get("created_at"),
                exact,
                matching_frames,
                score: if exact { 100 } else { score },
            });
        }

        matches.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| right.created_at.cmp(&left.created_at))
        });

        // Several reports can support one issue; show its best example once.
        let mut seen_issues = HashSet::<IssueId>::new();
        matches.retain(|candidate| {
            candidate
                .issue_id
                .is_none_or(|issue_id| seen_issues.insert(issue_id))
        });
        matches.truncate(8);
        Ok(matches)
    }
}

struct PendingStackTrace {
    report_id: ReportId,
    kind: String,
    key: String,
    text: String,
    signal: Option<String>,
    process: Option<String>,
}

fn compare_frames(left: &[String], right: &[String]) -> (usize, usize) {
    let left = &left[..left.len().min(12)];
    let right = &right[..right.len().min(12)];
    let mut previous = vec![0; right.len() + 1];
    let mut current = vec![0; right.len() + 1];
    for left_frame in left {
        for (index, right_frame) in right.iter().enumerate() {
            current[index + 1] = if left_frame == right_frame {
                previous[index] + 1
            } else {
                current[index].max(previous[index + 1])
            };
        }
        std::mem::swap(&mut current, &mut previous);
        current.fill(0);
    }
    let matching = previous[right.len()];
    let prefix = left
        .iter()
        .zip(right.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let score = (matching * 70 / left.len().max(right.len()).max(1) + prefix * 30 / 5).min(99);
    (matching, score)
}

#[cfg(test)]
mod tests {
    use super::compare_frames;

    #[test]
    fn ranks_ordered_frames_and_rewards_matching_top_frames() {
        let source = vec!["a".into(), "b".into(), "c".into()];
        let close = vec!["a".into(), "b".into(), "different".into()];
        let shifted = vec!["different".into(), "a".into(), "b".into()];
        assert!(compare_frames(&source, &close).1 > compare_frames(&source, &shifted).1);
    }
}
