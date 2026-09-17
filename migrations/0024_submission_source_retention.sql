ALTER TABLE reports DROP COLUMN source_ip;

ALTER TABLE reports ADD COLUMN source_client_key_expires_at timestamptz;

UPDATE reports
SET source_client_key_expires_at = created_at + interval '30 days'
WHERE source_client_key IS NOT NULL;

UPDATE reports
SET source_client_key = NULL,
    source_client_key_expires_at = NULL
WHERE source_client_key_expires_at <= now();

UPDATE runtime_configuration
SET value = jsonb_set(
    value,
    '{maintenance,submission_source_retention_days}',
    '30'::jsonb,
    true
);

DROP FUNCTION accept_report(
    uuid, text, text, uuid, uuid, text, text, text, uuid, text, inet, jsonb, jsonb
);

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
    source_retention_days integer,
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
    IF source_retention_days < 1 OR source_retention_days > 365 THEN
        RAISE EXCEPTION 'invalid submission source retention';
    END IF;

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
        UPDATE reports
        SET source_client_key_expires_at = created_at + make_interval(days => source_retention_days)
        WHERE id = result.report_id AND source_client_key IS NOT NULL;

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
    uuid, text, text, uuid, uuid, text, text, text, uuid, text, integer, jsonb, jsonb
) FROM PUBLIC;
