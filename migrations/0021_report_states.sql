ALTER TABLE reports ADD COLUMN state text;

UPDATE reports
SET state = CASE
    WHEN hidden_at IS NOT NULL OR deleted_at IS NOT NULL THEN 'rejected'
    WHEN confirmed_at IS NOT NULL OR issue_id IS NOT NULL THEN 'confirmed'
    ELSE 'triage'
END;

ALTER TABLE reports
    ALTER COLUMN state SET NOT NULL,
    ALTER COLUMN state SET DEFAULT 'triage',
    ADD CONSTRAINT reports_state_valid
        CHECK (state IN ('triage', 'confirmed', 'rejected')),
    ADD CONSTRAINT reports_linked_state_valid
        CHECK (issue_id IS NULL OR state <> 'triage');

DROP INDEX reports_triage;
DROP INDEX reports_visible;
DROP INDEX reports_expiry;
DROP INDEX reports_deletion;
DROP INDEX attachments_expiry;

ALTER TABLE reports
    DROP COLUMN confirmed_at,
    DROP COLUMN hidden_at,
    DROP COLUMN deleted_at;

ALTER TABLE attachments DROP COLUMN deleted_at;

CREATE INDEX reports_triage
    ON reports (created_at DESC, id DESC)
    WHERE state = 'triage' AND storage_state = 'ready';

CREATE INDEX reports_visible
    ON reports (created_at DESC, id DESC)
    WHERE state <> 'rejected' AND storage_state = 'ready';

CREATE INDEX reports_expiry ON reports (expires_at)
    WHERE expires_at IS NOT NULL;

CREATE INDEX attachments_expiry ON attachments (expires_at)
    WHERE expires_at IS NOT NULL;
