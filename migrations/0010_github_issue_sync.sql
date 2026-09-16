ALTER TABLE issues
    ADD COLUMN github_repository text,
    ADD COLUMN github_issue_id bigint,
    ADD COLUMN github_title text,
    ADD COLUMN github_state text NOT NULL DEFAULT 'unknown'
        CHECK (github_state IN ('unknown', 'open', 'closed', 'missing', 'moved', 'unavailable')),
    ADD COLUMN github_checked_at timestamptz,
    ADD COLUMN github_updated_at timestamptz;

UPDATE issues
SET
    github_repository = split_part(split_part(github_url, 'github.com/', 2), '/issues/', 1),
    github_title = title;

ALTER TABLE issues
    ALTER COLUMN github_repository SET NOT NULL,
    ALTER COLUMN github_title SET NOT NULL,
    ADD CONSTRAINT github_repository_name CHECK (
        github_repository ~ '^[^/]+/[^/]+$'
    );

DROP INDEX one_active_issue_per_github_issue;
CREATE UNIQUE INDEX one_active_issue_per_github_issue
    ON issues (lower(github_repository), github_number)
    WHERE merged_into IS NULL;

CREATE INDEX issues_by_github_id ON issues (github_issue_id)
    WHERE github_issue_id IS NOT NULL;
CREATE UNIQUE INDEX one_active_issue_per_github_id
    ON issues (github_issue_id)
    WHERE github_issue_id IS NOT NULL AND merged_into IS NULL;

CREATE TABLE issue_github_aliases (
    issue_id uuid NOT NULL REFERENCES issues CHECK (is_uuid_v7(issue_id)),
    github_repository text NOT NULL,
    github_number bigint NOT NULL CHECK (github_number > 0),
    github_issue_id bigint,
    github_url text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (github_repository, github_number)
);

CREATE INDEX issue_github_aliases_by_issue ON issue_github_aliases (issue_id);
