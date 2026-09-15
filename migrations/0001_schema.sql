CREATE TABLE runtime_configuration (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    value jsonb NOT NULL,
    revision bigint NOT NULL DEFAULT 1,
    updated_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO runtime_configuration (value)
VALUES (
    '{
      "public_base_url": "http://localhost:3001",
      "admin_base_url": "http://localhost:3000",
      "github_repository": "LadybirdBrowser/ladybird",
      "trusted_proxies": [],
      "limits": {
        "public_requests_per_minute": 120,
        "public_request_burst": 30,
        "challenges_per_minute": 10,
        "challenge_burst": 3,
        "challenges_per_hour": 100,
        "submissions_per_minute": 5,
        "submission_burst": 2,
        "submissions_per_hour": 50,
        "concurrent_uploads_per_ip": 2,
        "concurrent_uploads_global": 16,
        "maximum_fields": 64,
        "short_text_bytes": 4096,
        "multiline_text_bytes": 262144,
        "metadata_bytes": 1048576,
        "maximum_attachments": 5,
        "attachment_bytes": 10485760,
        "submission_bytes": 26214400,
        "png_pixels": 25000000,
        "upload_timeout_seconds": 120,
        "minimum_free_storage_bytes": 5368709120
      },
      "proof_of_work": {
        "expected_work": 5244236,
        "challenge_lifetime_seconds": 600
      },
      "membership_recheck_seconds": 300
    }'::jsonb
);

CREATE TABLE field_definitions (
    key text PRIMARY KEY CHECK (
        length(key) BETWEEN 1 AND 64
        AND key ~ '^[A-Za-z0-9._-]+$'
    ),
    label text NOT NULL CHECK (length(label) BETWEEN 1 AND 128),
    kind text NOT NULL CHECK (
        kind IN ('text', 'multiline', 'number', 'boolean', 'attachment')
    ),
    position integer NOT NULL DEFAULT 0
);

INSERT INTO field_definitions (key, label, kind, position)
VALUES
    ('stack', 'Native stack', 'multiline', 0),
    ('signal', 'Termination signal', 'text', 10),
    ('platform', 'Platform', 'text', 20),
    ('architecture', 'Architecture', 'text', 30),
    ('description', 'Description', 'multiline', 40),
    ('hostname', 'Website hostname', 'text', 50),
    ('url', 'Page URL', 'text', 60);

CREATE TABLE maintainers (
    github_id bigint PRIMARY KEY,
    login text NOT NULL CHECK (length(login) BETWEEN 1 AND 64),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sessions (
    token_hash text PRIMARY KEY CHECK (length(token_hash) = 64),
    github_id bigint NOT NULL REFERENCES maintainers,
    encrypted_access_token text NOT NULL,
    csrf_token text NOT NULL,
    membership_verified_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);

CREATE INDEX sessions_expiry ON sessions (expires_at);

CREATE TABLE oauth_states (
    state_hash text PRIMARY KEY CHECK (length(state_hash) = 64),
    expires_at timestamptz NOT NULL
);

CREATE INDEX oauth_states_expiry ON oauth_states (expires_at);

CREATE TABLE bootstrap_credentials (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    encrypted_reporting_database_url text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- All application and protocol identifiers use UUIDv7. The timestamp-ordered
-- layout keeps indexes well behaved and gives every producer one documented
-- identifier format.
CREATE FUNCTION is_uuid_v7(value uuid)
RETURNS boolean
LANGUAGE sql
IMMUTABLE
STRICT
AS $$
    SELECT
        substring(value::text FROM 15 FOR 1) = '7'
        AND substring(value::text FROM 20 FOR 1) IN ('8', '9', 'a', 'b');
$$;

CREATE TABLE issues (
    id uuid PRIMARY KEY CHECK (is_uuid_v7(id)),
    title text NOT NULL CHECK (length(title) BETWEEN 1 AND 256),
    description text NOT NULL DEFAULT '' CHECK (length(description) <= 262144),
    resolved_at timestamptz,
    merged_into uuid REFERENCES issues CHECK (is_uuid_v7(merged_into)),
    github_number bigint CHECK (github_number > 0),
    github_url text CHECK (github_url IS NULL OR length(github_url) <= 2048),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (merged_into IS NULL OR merged_into <> id),
    CHECK ((github_number IS NULL) = (github_url IS NULL))
);

CREATE TABLE reports (
    id uuid PRIMARY KEY CHECK (is_uuid_v7(id)),
    submission_id uuid NOT NULL UNIQUE CHECK (is_uuid_v7(submission_id)),
    manifest_digest text NOT NULL CHECK (manifest_digest ~ '^[0-9a-f]{64}$'),
    kind text NOT NULL CHECK (kind IN ('crash', 'web_compat')),
    client_version text NOT NULL CHECK (length(client_version) BETWEEN 1 AND 256),
    build text NOT NULL CHECK (length(build) <= 1024),
    storage_state text NOT NULL CHECK (storage_state IN ('pending_files', 'ready')),
    staging_id uuid NOT NULL UNIQUE CHECK (is_uuid_v7(staging_id)),
    source_client_key text CHECK (
        source_client_key IS NULL OR source_client_key ~ '^[0-9a-f]{64}$'
    ),
    issue_id uuid REFERENCES issues CHECK (is_uuid_v7(issue_id)),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    assigned_at timestamptz,
    expires_at timestamptz,
    deleted_at timestamptz,
    CHECK ((issue_id IS NULL) = (assigned_at IS NULL))
);

CREATE INDEX reports_triage
    ON reports (created_at DESC)
    WHERE issue_id IS NULL AND deleted_at IS NULL AND storage_state = 'ready';

CREATE INDEX reports_by_issue ON reports (issue_id);
CREATE INDEX reports_pending_storage ON reports (created_at) WHERE storage_state = 'pending_files';

-- Client addresses are HMACed before they reach this table. A NULL expiry is an
-- indefinite rate limit. Keeping lifted rows preserves the operational history
-- while allowing the same source to be blocked again later.
CREATE TABLE source_rate_limits (
    client_key text PRIMARY KEY CHECK (client_key ~ '^[0-9a-f]{64}$'),
    source_report_id uuid
        REFERENCES reports ON DELETE SET NULL
        CHECK (is_uuid_v7(source_report_id)),
    imposed_by bigint NOT NULL REFERENCES maintainers,
    imposed_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz,
    lifted_by bigint REFERENCES maintainers,
    lifted_at timestamptz,
    CHECK ((lifted_by IS NULL) = (lifted_at IS NULL))
);

CREATE TABLE report_fields (
    report_id uuid NOT NULL
        REFERENCES reports ON DELETE CASCADE
        CHECK (is_uuid_v7(report_id)),
    key text NOT NULL CHECK (
        length(key) BETWEEN 1 AND 64
        AND key ~ '^[A-Za-z0-9._-]+$'
    ),
    kind text NOT NULL CHECK (
        kind IN ('text', 'multiline', 'number', 'boolean', 'attachment')
    ),
    value jsonb NOT NULL,
    recognized_at_submission boolean NOT NULL,
    PRIMARY KEY (report_id, key)
);

CREATE TABLE attachments (
    id uuid PRIMARY KEY CHECK (is_uuid_v7(id)),
    report_id uuid NOT NULL
        REFERENCES reports ON DELETE CASCADE
        CHECK (is_uuid_v7(report_id)),
    client_id uuid NOT NULL CHECK (is_uuid_v7(client_id)),
    name text NOT NULL CHECK (length(name) BETWEEN 1 AND 128),
    media_type text NOT NULL CHECK (media_type IN ('image/png', 'text/plain')),
    size bigint NOT NULL CHECK (size BETWEEN 0 AND 67108864),
    sha256 text NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    storage_key text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz,
    deleted_at timestamptz,
    UNIQUE (report_id, client_id)
);

CREATE TABLE challenges (
    id uuid PRIMARY KEY CHECK (is_uuid_v7(id)),
    manifest_digest text NOT NULL CHECK (manifest_digest ~ '^[0-9a-f]{64}$'),
    token_hash text NOT NULL CHECK (length(token_hash) = 64),
    expires_at timestamptz NOT NULL,
    report_id uuid REFERENCES reports CHECK (is_uuid_v7(report_id)),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX challenges_expiry ON challenges (expires_at);

CREATE TABLE rate_buckets (
    key text PRIMARY KEY,
    tokens double precision NOT NULL,
    updated_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL
);

CREATE INDEX rate_buckets_expiry ON rate_buckets (expires_at);

CREATE TABLE upload_leases (
    id uuid PRIMARY KEY CHECK (is_uuid_v7(id)),
    ip_key text NOT NULL,
    expires_at timestamptz NOT NULL
);

CREATE INDEX upload_leases_expiry ON upload_leases (expires_at);

CREATE TABLE audit_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor bigint REFERENCES maintainers,
    action text NOT NULL,
    entity_id uuid CHECK (is_uuid_v7(entity_id)),
    details jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX audit_events_entity ON audit_events (entity_id, id DESC);

CREATE TABLE github_publish_attempts (
    id uuid PRIMARY KEY CHECK (is_uuid_v7(id)),
    issue_id uuid NOT NULL REFERENCES issues CHECK (is_uuid_v7(issue_id)),
    actor bigint NOT NULL REFERENCES maintainers,
    state text NOT NULL CHECK (state IN ('pending', 'succeeded', 'uncertain', 'failed')),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX one_unresolved_github_publish
    ON github_publish_attempts (issue_id)
    WHERE state IN ('pending', 'uncertain');
