-- A report's submission source identifier is now cleared a configurable number of
-- hours after the report was created (48 by default), counted from created_at by
-- the maintenance sweep. The per-report expiry timestamp and the retention
-- argument of accept_report are no longer needed. The old setting, in days, is
-- replaced by one in hours. Grants are reapplied by the admin service on startup.

UPDATE runtime_configuration
SET value = jsonb_set(
    value #- '{maintenance,submission_source_retention_days}',
    '{maintenance,submission_source_retention_hours}',
    '48'
)
WHERE value #> '{maintenance}' IS NOT NULL;

DROP FUNCTION public.accept_report(uuid, text, text, uuid, uuid, text, text, text, uuid, text, integer, jsonb, jsonb);

CREATE FUNCTION public.accept_report(challenge_id uuid, signed_token_hash text, digest text, new_report_id uuid, submission uuid, report_kind text, version text, build_description text, upload_staging_id uuid, report_source_client_key text, diagnostic_fields jsonb, attachment_metadata jsonb) RETURNS public.accept_report_result
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
DECLARE
    existing_report reports%ROWTYPE;
    field_record jsonb;
    attachment_record jsonb;
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended(submission::text, 0));

    SELECT *
    INTO existing_report
    FROM reports
    WHERE submission_id = submission;

    IF FOUND THEN
        IF existing_report.manifest_digest = digest THEN
            RETURN ROW(existing_report.id, 'existing')::accept_report_result;
        END IF;

        RETURN ROW(existing_report.id, 'submission_conflict')::accept_report_result;
    END IF;

    PERFORM 1
    FROM challenges
    WHERE id = challenge_id
        AND manifest_digest = digest
        AND token_hash = signed_token_hash
        AND report_id IS NULL
        AND expires_at > now()
    FOR UPDATE;

    IF NOT FOUND THEN
        RETURN ROW(NULL, 'challenge_rejected')::accept_report_result;
    END IF;

    IF jsonb_typeof(diagnostic_fields) <> 'array'
        OR jsonb_array_length(diagnostic_fields) > 256
        OR jsonb_typeof(attachment_metadata) <> 'array'
        OR jsonb_array_length(attachment_metadata) > 16
    THEN
        RETURN ROW(NULL, 'invalid_payload')::accept_report_result;
    END IF;

    INSERT INTO reports (
        id,
        submission_id,
        manifest_digest,
        kind,
        client_version,
        build,
        storage_state,
        staging_id,
        source_client_key
    )
    VALUES (
        new_report_id,
        submission,
        digest,
        report_kind,
        version,
        build_description,
        'pending_files',
        upload_staging_id,
        report_source_client_key
    );

    UPDATE challenges SET report_id = new_report_id WHERE id = challenge_id;

    FOR field_record IN SELECT value FROM jsonb_array_elements(diagnostic_fields)
    LOOP
        INSERT INTO report_fields (
            report_id,
            key,
            kind,
            value,
            recognized_at_submission
        )
        VALUES (
            new_report_id,
            field_record->>'key',
            field_record->>'kind',
            field_record->'value',
            (field_record->>'recognized')::boolean
        );
    END LOOP;

    FOR attachment_record IN SELECT value FROM jsonb_array_elements(attachment_metadata)
    LOOP
        INSERT INTO attachments (
            id,
            report_id,
            client_reference,
            name,
            media_type,
            size,
            sha256,
            storage_key
        )
        VALUES (
            (attachment_record->>'id')::uuid,
            new_report_id,
            attachment_record->>'client_reference',
            attachment_record->>'name',
            attachment_record->>'media_type',
            (attachment_record->>'size')::bigint,
            attachment_record->>'sha256',
            'reports/' || new_report_id::text || '/' || (attachment_record->>'id')
        );
    END LOOP;

    INSERT INTO audit_events (action, entity_id, details)
    VALUES ('report.submitted', new_report_id, jsonb_build_object('kind', report_kind));

    RETURN ROW(new_report_id, 'accepted')::accept_report_result;
END;
$$;

ALTER TABLE reports DROP COLUMN source_client_key_expires_at;
