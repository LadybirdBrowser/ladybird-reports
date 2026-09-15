CREATE FUNCTION issue_challenge(
    challenge_id uuid,
    digest text,
    signed_token_hash text,
    challenge_expires_at timestamptz
)
RETURNS void
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    INSERT INTO challenges (id, manifest_digest, token_hash, expires_at)
    VALUES (challenge_id, digest, signed_token_hash, challenge_expires_at);
$$;

CREATE FUNCTION consume_rate_limit(
    bucket_key text,
    refill_count double precision,
    refill_seconds double precision,
    capacity double precision
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
DECLARE
    available_tokens double precision;
    previous_update timestamptz;
    observed_at timestamptz := clock_timestamp();
BEGIN
    INSERT INTO rate_buckets (key, tokens, updated_at, expires_at)
    VALUES (bucket_key, capacity, observed_at, observed_at + interval '2 hours')
    ON CONFLICT DO NOTHING;

    SELECT tokens, updated_at
    INTO available_tokens, previous_update
    FROM rate_buckets
    WHERE key = bucket_key
    FOR UPDATE;

    available_tokens := least(
        capacity,
        available_tokens
            + greatest(0, extract(epoch FROM observed_at - previous_update))
            * refill_count
            / refill_seconds
    );

    UPDATE rate_buckets
    SET
        tokens = greatest(0, available_tokens - 1),
        updated_at = observed_at,
        expires_at = observed_at + interval '2 hours'
    WHERE key = bucket_key;

    RETURN available_tokens >= 1;
END;
$$;

CREATE FUNCTION source_has_active_rate_limit(candidate_client_key text)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
STABLE
SET search_path = public, pg_temp
AS $$
    SELECT EXISTS(
        SELECT 1
        FROM source_rate_limits
        WHERE client_key = candidate_client_key
            AND lifted_at IS NULL
            AND (expires_at IS NULL OR expires_at > now())
    );
$$;

CREATE FUNCTION acquire_upload_lease(
    lease_id uuid,
    client_key text,
    per_client_limit integer,
    global_limit integer,
    lifetime_seconds integer
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(891124);

    DELETE FROM upload_leases WHERE expires_at < now();

    IF (SELECT count(*) FROM upload_leases) >= global_limit THEN
        RETURN false;
    END IF;

    IF (SELECT count(*) FROM upload_leases WHERE ip_key = client_key) >= per_client_limit THEN
        RETURN false;
    END IF;

    INSERT INTO upload_leases (id, ip_key, expires_at)
    VALUES (lease_id, client_key, now() + make_interval(secs => lifetime_seconds));

    RETURN true;
END;
$$;

CREATE FUNCTION release_upload_lease(lease_id uuid)
RETURNS void
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    DELETE FROM upload_leases WHERE id = lease_id;
$$;

CREATE TYPE report_receipt AS (
    report_id uuid,
    digest_matches boolean,
    storage_state text,
    staging_id uuid
);

CREATE FUNCTION find_report_receipt(submission uuid, digest text)
RETURNS report_receipt
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT ROW(
        reports.id,
        reports.manifest_digest = digest,
        reports.storage_state,
        reports.staging_id
    )::report_receipt
    FROM reports
    WHERE reports.submission_id = submission;
$$;

CREATE TYPE accept_report_result AS (
    report_id uuid,
    outcome text
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
    diagnostic_fields jsonb,
    attachment_metadata jsonb
)
RETURNS accept_report_result
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
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
            client_id,
            name,
            media_type,
            size,
            sha256,
            storage_key
        )
        VALUES (
            (attachment_record->>'id')::uuid,
            new_report_id,
            (attachment_record->>'client_id')::uuid,
            attachment_record->>'name',
            attachment_record->>'media_type',
            (attachment_record->>'size')::bigint,
            attachment_record->>'sha256',
            'reports/' || new_report_id::text || '/' || (attachment_record->>'client_id')
        );
    END LOOP;

    RETURN ROW(new_report_id, 'accepted')::accept_report_result;
END;
$$;

CREATE FUNCTION mark_report_storage_ready(report uuid, upload_staging_id uuid)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    WITH updated AS (
        UPDATE reports
        SET storage_state = 'ready', updated_at = now()
        WHERE id = report
            AND staging_id = upload_staging_id
            AND storage_state = 'pending_files'
        RETURNING 1
    )
    SELECT EXISTS(SELECT 1 FROM updated);
$$;

CREATE FUNCTION pending_report_storage()
RETURNS TABLE (report_id uuid, staging_id uuid)
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT id, reports.staging_id
    FROM reports
    WHERE storage_state = 'pending_files'
    ORDER BY created_at
    LIMIT 100;
$$;

CREATE FUNCTION staging_upload_is_referenced(upload_staging_id uuid)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT EXISTS(SELECT 1 FROM reports WHERE staging_id = upload_staging_id);
$$;

REVOKE ALL ON FUNCTION issue_challenge(uuid, text, text, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION consume_rate_limit(text, double precision, double precision, double precision) FROM PUBLIC;
REVOKE ALL ON FUNCTION source_has_active_rate_limit(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION acquire_upload_lease(uuid, text, integer, integer, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION release_upload_lease(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION find_report_receipt(uuid, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION accept_report(
    uuid, text, text, uuid, uuid, text, text, text, uuid, text, jsonb, jsonb
) FROM PUBLIC;
REVOKE ALL ON FUNCTION mark_report_storage_ready(uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION pending_report_storage() FROM PUBLIC;
REVOKE ALL ON FUNCTION staging_upload_is_referenced(uuid) FROM PUBLIC;
