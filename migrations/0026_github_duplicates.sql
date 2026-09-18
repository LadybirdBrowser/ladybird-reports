CREATE TABLE github_duplicate_jobs (
    source_issue_id uuid PRIMARY KEY REFERENCES issues(id) ON DELETE CASCADE,
    github_issue_id bigint NOT NULL,
    github_repository text NOT NULL,
    github_number bigint NOT NULL,
    installation_id bigint NOT NULL,
    lease_id uuid CHECK (lease_id IS NULL OR is_uuid_v7(lease_id)),
    lease_expires_at timestamptz,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    attempt_count integer NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((lease_id IS NULL) = (lease_expires_at IS NULL))
);

CREATE INDEX github_duplicate_jobs_pending
    ON github_duplicate_jobs (next_attempt_at)
    WHERE lease_id IS NULL;
