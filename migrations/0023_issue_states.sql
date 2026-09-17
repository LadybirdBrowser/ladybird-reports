ALTER TABLE issues ADD COLUMN state text;

UPDATE issues SET state = CASE
    WHEN hidden_at IS NOT NULL THEN 'rejected'
    WHEN github_state IN ('missing', 'moved', 'unavailable') THEN 'needs_attention'
    WHEN github_state = 'closed' THEN 'resolved'
    ELSE 'unresolved'
END;

ALTER TABLE issues
    ALTER COLUMN state SET NOT NULL,
    ALTER COLUMN state SET DEFAULT 'unresolved',
    ADD CONSTRAINT issues_state_valid
        CHECK (state IN ('unresolved', 'needs_attention', 'resolved', 'rejected'));

DROP INDEX issues_visible;
DROP INDEX one_active_issue_per_github_issue;
DROP INDEX one_active_issue_per_github_id;

ALTER TABLE issues DROP COLUMN hidden_at;

CREATE INDEX issues_visible ON issues (updated_at DESC)
    WHERE state <> 'rejected';

CREATE UNIQUE INDEX one_active_issue_per_github_issue
    ON issues (lower(github_repository), github_number)
    WHERE merged_into IS NULL AND state <> 'rejected';

CREATE UNIQUE INDEX one_active_issue_per_github_id
    ON issues (github_issue_id)
    WHERE github_issue_id IS NOT NULL
        AND merged_into IS NULL
        AND state <> 'rejected';
