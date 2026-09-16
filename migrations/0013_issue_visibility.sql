ALTER TABLE issues ADD COLUMN hidden_at timestamptz;

CREATE INDEX issues_visible
    ON issues (updated_at DESC)
    WHERE hidden_at IS NULL;

DROP INDEX one_active_issue_per_github_issue;
CREATE UNIQUE INDEX one_active_issue_per_github_issue
    ON issues (lower(github_repository), github_number)
    WHERE merged_into IS NULL AND hidden_at IS NULL;

DROP INDEX one_active_issue_per_github_id;
CREATE UNIQUE INDEX one_active_issue_per_github_id
    ON issues (github_issue_id)
    WHERE github_issue_id IS NOT NULL
        AND merged_into IS NULL
        AND hidden_at IS NULL;
