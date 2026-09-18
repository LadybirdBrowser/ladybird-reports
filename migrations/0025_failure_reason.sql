INSERT INTO field_definitions (key, label, kind, position)
VALUES ('failure_reason', 'Failure reason', 'text', 9)
ON CONFLICT (key) DO NOTHING;

ALTER TABLE attachments
    ADD COLUMN failure_reason_processed_at timestamptz;

CREATE INDEX attachments_pending_failure_reason
    ON attachments (created_at)
    WHERE failure_reason_processed_at IS NULL
        AND name = 'crash-diagnostics.txt'
        AND media_type = 'text/plain';
