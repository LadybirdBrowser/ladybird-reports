ALTER TABLE field_definitions
    DROP CONSTRAINT field_definitions_kind_check;
ALTER TABLE field_definitions
    ADD CONSTRAINT field_definitions_kind_check
    CHECK (kind IN ('text', 'multiline', 'stack_trace', 'number', 'boolean', 'attachment'));

ALTER TABLE report_fields
    DROP CONSTRAINT report_fields_kind_check;
ALTER TABLE report_fields
    ADD CONSTRAINT report_fields_kind_check
    CHECK (kind IN ('text', 'multiline', 'stack_trace', 'number', 'boolean', 'attachment'));

UPDATE field_definitions
SET kind = 'stack_trace'
WHERE key = 'stack' AND kind = 'multiline';

INSERT INTO field_definitions (key, label, kind, position)
VALUES ('process', 'Process', 'text', 35)
ON CONFLICT (key) DO NOTHING;

CREATE TABLE report_stack_signatures (
    report_id uuid NOT NULL,
    field_key text NOT NULL,
    algorithm_version integer NOT NULL,
    status text NOT NULL CHECK (status IN ('parsed', 'insufficient')),
    fingerprint text CHECK (fingerprint IS NULL OR fingerprint ~ '^[0-9a-f]{64}$'),
    frame_keys text[] NOT NULL,
    indexed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (report_id, field_key),
    FOREIGN KEY (report_id, field_key)
        REFERENCES report_fields (report_id, key) ON DELETE CASCADE
);

CREATE INDEX report_stack_fingerprint
    ON report_stack_signatures (fingerprint)
    WHERE fingerprint IS NOT NULL;

CREATE INDEX report_stack_frame_keys
    ON report_stack_signatures USING gin (frame_keys);

CREATE INDEX report_fields_stack_index_candidates
    ON report_fields (report_id, key)
    WHERE kind = 'stack_trace' OR (key = 'stack' AND kind = 'multiline');
