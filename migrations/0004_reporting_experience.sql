ALTER TABLE runtime_configuration DROP COLUMN revision;

UPDATE runtime_configuration
SET value = jsonb_set(
    value,
    '{public_base_url}',
    to_jsonb('https://reports.app.ladybird.org'::text)
)
WHERE value->>'public_base_url' = 'http://localhost:3001';

ALTER TABLE reports ADD COLUMN source_ip inet;

ALTER FUNCTION accept_report(
    uuid, text, text, uuid, uuid, text, text, text, uuid, text, jsonb, jsonb
) RENAME TO accept_report_without_source_address;

REVOKE ALL ON FUNCTION accept_report_without_source_address(
    uuid, text, text, uuid, uuid, text, text, text, uuid, text, jsonb, jsonb
) FROM PUBLIC;

CREATE FUNCTION accept_report(
    challenge_id uuid,
    signed_token_hash text,
    digest text,
    new_report_id uuid,
    submission uuid,
    report_kind text,
    version text,
    build_description text,
    upload_staging_id uuid,
    report_source_client_key text,
    report_source_ip inet,
    diagnostic_fields jsonb,
    attachment_metadata jsonb
)
RETURNS accept_report_result
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    result accept_report_result;
BEGIN
    result := accept_report_without_source_address(
        challenge_id,
        signed_token_hash,
        digest,
        new_report_id,
        submission,
        report_kind,
        version,
        build_description,
        upload_staging_id,
        report_source_client_key,
        diagnostic_fields,
        attachment_metadata
    );

    IF result.outcome = 'accepted' THEN
        UPDATE reports SET source_ip = report_source_ip WHERE id = result.report_id;

        INSERT INTO audit_events (action, entity_id, details)
        VALUES (
            'report.submitted',
            result.report_id,
            jsonb_build_object('kind', report_kind)
        );
    END IF;

    RETURN result;
END;
$$;

REVOKE ALL ON FUNCTION accept_report(
    uuid, text, text, uuid, uuid, text, text, text, uuid, text, inet, jsonb, jsonb
) FROM PUBLIC;

INSERT INTO audit_events (action, entity_id, details, created_at)
SELECT
    'report.submitted',
    reports.id,
    jsonb_build_object('kind', reports.kind),
    reports.created_at
FROM reports
WHERE NOT EXISTS (
    SELECT 1
    FROM audit_events
    WHERE audit_events.entity_id = reports.id
        AND audit_events.action = 'report.submitted'
);
