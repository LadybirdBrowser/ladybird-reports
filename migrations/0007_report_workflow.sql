ALTER TABLE reports
    ADD COLUMN confirmed_at timestamptz,
    ADD COLUMN hidden_at timestamptz;

DROP INDEX reports_triage;

CREATE INDEX reports_triage
    ON reports (created_at DESC, id DESC)
    WHERE issue_id IS NULL
        AND confirmed_at IS NULL
        AND hidden_at IS NULL
        AND deleted_at IS NULL
        AND storage_state = 'ready';

CREATE INDEX reports_visible
    ON reports (created_at DESC, id DESC)
    WHERE hidden_at IS NULL
        AND deleted_at IS NULL
        AND storage_state = 'ready';
