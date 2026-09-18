-- Baseline schema after migration 25. Future changes start at version 26.
-- Generated from a fresh PostgreSQL 17 database after applying migrations 1-25.
SET LOCAL check_function_bodies = false;

-- Name: accept_report_result; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.accept_report_result AS (
	report_id uuid,
	outcome text
);


--
-- Name: report_receipt; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.report_receipt AS (
	report_id uuid,
	digest_matches boolean,
	storage_state text,
	staging_id uuid
);


--
-- Name: accept_report(uuid, text, text, uuid, uuid, text, text, text, uuid, text, integer, jsonb, jsonb); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.accept_report(challenge_id uuid, signed_token_hash text, digest text, new_report_id uuid, submission uuid, report_kind text, version text, build_description text, upload_staging_id uuid, report_source_client_key text, source_retention_days integer, diagnostic_fields jsonb, attachment_metadata jsonb) RETURNS public.accept_report_result
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
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


--
-- Name: accept_report_without_source_address(uuid, text, text, uuid, uuid, text, text, text, uuid, text, jsonb, jsonb); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.accept_report_without_source_address(challenge_id uuid, signed_token_hash text, digest text, new_report_id uuid, submission uuid, report_kind text, version text, build_description text, upload_staging_id uuid, report_source_client_key text, diagnostic_fields jsonb, attachment_metadata jsonb) RETURNS public.accept_report_result
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


--
-- Name: acquire_upload_lease(uuid, text, integer, integer, integer); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.acquire_upload_lease(lease_id uuid, client_key text, per_client_limit integer, global_limit integer, lifetime_seconds integer) RETURNS boolean
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
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


--
-- Name: configure_report_retention(uuid, integer); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.configure_report_retention(report uuid, retention_days integer) RETURNS void
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
BEGIN
    IF retention_days < 30 OR retention_days > 36500 THEN
        RAISE EXCEPTION 'invalid report retention';
    END IF;

    UPDATE reports
    SET expires_at = created_at + make_interval(days => retention_days)
    WHERE id = report AND expires_at IS NULL;

    UPDATE attachments
    SET expires_at = created_at + make_interval(days => retention_days)
    WHERE report_id = report AND expires_at IS NULL;
END;
$$;


--
-- Name: consume_rate_limit(text, double precision, double precision, double precision); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.consume_rate_limit(bucket_key text, refill_count double precision, refill_seconds double precision, capacity double precision) RETURNS boolean
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
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


--
-- Name: enqueue_discord_report_notification(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.enqueue_discord_report_notification() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
BEGIN
    IF NEW.storage_state = 'ready' THEN
        INSERT INTO discord_report_notifications (report_id)
        VALUES (NEW.id)
        ON CONFLICT (report_id) DO NOTHING;
    END IF;

    RETURN NEW;
END;
$$;


--
-- Name: find_report_receipt(uuid, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.find_report_receipt(submission uuid, digest text) RETURNS public.report_receipt
    LANGUAGE sql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
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


--
-- Name: is_uuid_v7(uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.is_uuid_v7(value uuid) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT
    AS $$
    SELECT
        substring(value::text FROM 15 FOR 1) = '7'
        AND substring(value::text FROM 20 FOR 1) IN ('8', '9', 'a', 'b');
$$;


--
-- Name: issue_challenge(uuid, text, text, timestamp with time zone); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.issue_challenge(challenge_id uuid, digest text, signed_token_hash text, challenge_expires_at timestamp with time zone) RETURNS void
    LANGUAGE sql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
    INSERT INTO challenges (id, manifest_digest, token_hash, expires_at)
    VALUES (challenge_id, digest, signed_token_hash, challenge_expires_at);
$$;


--
-- Name: mark_report_storage_ready(uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.mark_report_storage_ready(report uuid, upload_staging_id uuid) RETURNS boolean
    LANGUAGE sql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
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


--
-- Name: notify_runtime_configuration_change(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.notify_runtime_configuration_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_notify('ladybird_reports_configuration', '');
    RETURN NEW;
END;
$$;


--
-- Name: pending_report_storage(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.pending_report_storage() RETURNS TABLE(report_id uuid, staging_id uuid)
    LANGUAGE sql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
    SELECT id, reports.staging_id
    FROM reports
    WHERE storage_state = 'pending_files'
    ORDER BY created_at
    LIMIT 100;
$$;


--
-- Name: release_upload_lease(uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.release_upload_lease(lease_id uuid) RETURNS void
    LANGUAGE sql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
    DELETE FROM upload_leases WHERE id = lease_id;
$$;


--
-- Name: reporting_runtime_configuration(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.reporting_runtime_configuration() RETURNS jsonb
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
    SELECT value - 'discord'
    FROM runtime_configuration
    WHERE singleton = true;
$$;


--
-- Name: source_has_active_rate_limit(text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.source_has_active_rate_limit(candidate_client_key text) RETURNS boolean
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
    SELECT EXISTS(
        SELECT 1
        FROM source_rate_limits
        WHERE client_key = candidate_client_key
            AND lifted_at IS NULL
            AND (expires_at IS NULL OR expires_at > now())
    );
$$;


--
-- Name: staging_upload_is_referenced(uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.staging_upload_is_referenced(upload_staging_id uuid) RETURNS boolean
    LANGUAGE sql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
    SELECT EXISTS(SELECT 1 FROM reports WHERE staging_id = upload_staging_id);
$$;


--
-- Name: sweep_expired_ingestion_state(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.sweep_expired_ingestion_state() RETURNS TABLE(challenges_deleted bigint, rate_buckets_deleted bigint, upload_leases_deleted bigint)
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
DECLARE
    deleted_challenges bigint;
    deleted_rate_buckets bigint;
    deleted_upload_leases bigint;
BEGIN
    DELETE FROM challenges WHERE expires_at <= now();
    GET DIAGNOSTICS deleted_challenges = ROW_COUNT;

    DELETE FROM rate_buckets WHERE expires_at <= now();
    GET DIAGNOSTICS deleted_rate_buckets = ROW_COUNT;

    DELETE FROM upload_leases WHERE expires_at <= now();
    GET DIAGNOSTICS deleted_upload_leases = ROW_COUNT;

    RETURN QUERY SELECT deleted_challenges, deleted_rate_buckets, deleted_upload_leases;
END;
$$;


SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: attachments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.attachments (
    id uuid NOT NULL,
    report_id uuid NOT NULL,
    client_id uuid NOT NULL,
    name text NOT NULL,
    media_type text NOT NULL,
    size bigint NOT NULL,
    sha256 text NOT NULL,
    storage_key text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone,
    failure_reason_processed_at timestamp with time zone,
    CONSTRAINT attachments_client_id_check CHECK (public.is_uuid_v7(client_id)),
    CONSTRAINT attachments_id_check CHECK (public.is_uuid_v7(id)),
    CONSTRAINT attachments_media_type_check CHECK ((media_type = ANY (ARRAY['image/png'::text, 'text/plain'::text]))),
    CONSTRAINT attachments_name_check CHECK (((length(name) >= 1) AND (length(name) <= 128))),
    CONSTRAINT attachments_report_id_check CHECK (public.is_uuid_v7(report_id)),
    CONSTRAINT attachments_sha256_check CHECK ((sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT attachments_size_check CHECK (((size >= 0) AND (size <= 67108864)))
);


--
-- Name: audit_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.audit_events (
    id bigint NOT NULL,
    actor bigint,
    action text NOT NULL,
    entity_id uuid,
    details jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT audit_events_entity_id_check CHECK (public.is_uuid_v7(entity_id))
);


--
-- Name: audit_events_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.audit_events ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.audit_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);


--
-- Name: bootstrap_credentials; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.bootstrap_credentials (
    singleton boolean DEFAULT true NOT NULL,
    encrypted_reporting_database_url text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT bootstrap_credentials_singleton_check CHECK (singleton)
);


--
-- Name: challenges; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.challenges (
    id uuid NOT NULL,
    manifest_digest text NOT NULL,
    token_hash text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    report_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT challenges_id_check CHECK (public.is_uuid_v7(id)),
    CONSTRAINT challenges_manifest_digest_check CHECK ((manifest_digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT challenges_report_id_check CHECK (public.is_uuid_v7(report_id)),
    CONSTRAINT challenges_token_hash_check CHECK ((length(token_hash) = 64))
);


--
-- Name: discord_delivery_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.discord_delivery_state (
    singleton boolean DEFAULT true NOT NULL,
    lease_id uuid,
    leased_report_id uuid,
    lease_expires_at timestamp with time zone,
    paused_until timestamp with time zone,
    consecutive_failures integer DEFAULT 0 NOT NULL,
    CONSTRAINT discord_delivery_state_check CHECK ((((lease_id IS NULL) AND (leased_report_id IS NULL) AND (lease_expires_at IS NULL)) OR ((lease_id IS NOT NULL) AND (leased_report_id IS NOT NULL) AND (lease_expires_at IS NOT NULL)))),
    CONSTRAINT discord_delivery_state_consecutive_failures_check CHECK ((consecutive_failures >= 0)),
    CONSTRAINT discord_delivery_state_lease_id_check CHECK (public.is_uuid_v7(lease_id)),
    CONSTRAINT discord_delivery_state_leased_report_id_check CHECK (public.is_uuid_v7(leased_report_id)),
    CONSTRAINT discord_delivery_state_singleton_check CHECK (singleton)
);


--
-- Name: discord_report_notifications; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.discord_report_notifications (
    report_id uuid NOT NULL,
    attempt_count integer DEFAULT 0 NOT NULL,
    last_failure text,
    last_response_status smallint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    delivered_at timestamp with time zone,
    CONSTRAINT discord_report_notifications_attempt_count_check CHECK ((attempt_count >= 0)),
    CONSTRAINT discord_report_notifications_last_failure_check CHECK (((last_failure IS NULL) OR (length(last_failure) <= 128))),
    CONSTRAINT discord_report_notifications_last_response_status_check CHECK (((last_response_status IS NULL) OR ((last_response_status >= 100) AND (last_response_status <= 599)))),
    CONSTRAINT discord_report_notifications_report_id_check CHECK (public.is_uuid_v7(report_id))
);


--
-- Name: field_definitions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.field_definitions (
    key text NOT NULL,
    label text NOT NULL,
    kind text NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    CONSTRAINT field_definitions_key_check CHECK ((((length(key) >= 1) AND (length(key) <= 64)) AND (key ~ '^[A-Za-z0-9._-]+$'::text))),
    CONSTRAINT field_definitions_kind_check CHECK ((kind = ANY (ARRAY['text'::text, 'multiline'::text, 'stack_trace'::text, 'number'::text, 'boolean'::text, 'attachment'::text]))),
    CONSTRAINT field_definitions_label_check CHECK (((length(label) >= 1) AND (length(label) <= 128)))
);


--
-- Name: issue_github_aliases; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.issue_github_aliases (
    issue_id uuid NOT NULL,
    github_repository text NOT NULL,
    github_number bigint NOT NULL,
    github_issue_id bigint,
    github_url text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT issue_github_aliases_github_number_check CHECK ((github_number > 0)),
    CONSTRAINT issue_github_aliases_issue_id_check CHECK (public.is_uuid_v7(issue_id))
);


--
-- Name: issues; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.issues (
    id uuid NOT NULL,
    title text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    resolved_at timestamp with time zone,
    merged_into uuid,
    github_number bigint NOT NULL,
    github_url text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    github_repository text NOT NULL,
    github_issue_id bigint,
    github_state text DEFAULT 'unknown'::text NOT NULL,
    github_checked_at timestamp with time zone,
    github_updated_at timestamp with time zone,
    github_reports_field_id bigint,
    github_reports_link_url text,
    state text DEFAULT 'unresolved'::text NOT NULL,
    CONSTRAINT github_repository_name CHECK ((github_repository ~ '^[^/]+/[^/]+$'::text)),
    CONSTRAINT issues_check CHECK (((merged_into IS NULL) OR (merged_into <> id))),
    CONSTRAINT issues_check1 CHECK (((github_number IS NULL) = (github_url IS NULL))),
    CONSTRAINT issues_description_check CHECK ((length(description) <= 262144)),
    CONSTRAINT issues_github_number_check CHECK ((github_number > 0)),
    CONSTRAINT issues_github_state_check CHECK ((github_state = ANY (ARRAY['unknown'::text, 'open'::text, 'closed'::text, 'missing'::text, 'moved'::text, 'unavailable'::text]))),
    CONSTRAINT issues_github_url_check CHECK (((github_url IS NULL) OR (length(github_url) <= 2048))),
    CONSTRAINT issues_id_check CHECK (public.is_uuid_v7(id)),
    CONSTRAINT issues_merged_into_check CHECK (public.is_uuid_v7(merged_into)),
    CONSTRAINT issues_state_valid CHECK ((state = ANY (ARRAY['unresolved'::text, 'needs_attention'::text, 'resolved'::text, 'rejected'::text]))),
    CONSTRAINT issues_title_check CHECK (((length(title) >= 1) AND (length(title) <= 256)))
);


--
-- Name: maintainers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.maintainers (
    github_id bigint NOT NULL,
    login text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT maintainers_login_check CHECK (((length(login) >= 1) AND (length(login) <= 64)))
);


--
-- Name: oauth_states; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.oauth_states (
    state_hash text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    return_to text DEFAULT '/'::text NOT NULL,
    CONSTRAINT oauth_states_return_to_check CHECK (((length(return_to) <= 2048) AND (return_to ~~ '/%'::text) AND (return_to !~~ '//%'::text))),
    CONSTRAINT oauth_states_state_hash_check CHECK ((length(state_hash) = 64))
);


--
-- Name: rate_buckets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.rate_buckets (
    key text NOT NULL,
    tokens double precision NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL
);


--
-- Name: report_fields; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.report_fields (
    report_id uuid NOT NULL,
    key text NOT NULL,
    kind text NOT NULL,
    value jsonb NOT NULL,
    recognized_at_submission boolean NOT NULL,
    CONSTRAINT report_fields_key_check CHECK ((((length(key) >= 1) AND (length(key) <= 64)) AND (key ~ '^[A-Za-z0-9._-]+$'::text))),
    CONSTRAINT report_fields_kind_check CHECK ((kind = ANY (ARRAY['text'::text, 'multiline'::text, 'stack_trace'::text, 'number'::text, 'boolean'::text, 'attachment'::text]))),
    CONSTRAINT report_fields_report_id_check CHECK (public.is_uuid_v7(report_id))
);


--
-- Name: report_stack_signatures; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.report_stack_signatures (
    report_id uuid NOT NULL,
    field_key text NOT NULL,
    algorithm_version integer NOT NULL,
    status text NOT NULL,
    fingerprint text,
    frame_keys text[] NOT NULL,
    indexed_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT report_stack_signatures_fingerprint_check CHECK (((fingerprint IS NULL) OR (fingerprint ~ '^[0-9a-f]{64}$'::text))),
    CONSTRAINT report_stack_signatures_status_check CHECK ((status = ANY (ARRAY['parsed'::text, 'insufficient'::text])))
);


--
-- Name: reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.reports (
    id uuid NOT NULL,
    submission_id uuid NOT NULL,
    manifest_digest text NOT NULL,
    kind text NOT NULL,
    client_version text NOT NULL,
    build text NOT NULL,
    storage_state text NOT NULL,
    staging_id uuid NOT NULL,
    source_client_key text,
    issue_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone,
    auto_match_eligible boolean DEFAULT true NOT NULL,
    state text DEFAULT 'triage'::text NOT NULL,
    source_client_key_expires_at timestamp with time zone,
    CONSTRAINT reports_build_check CHECK ((length(build) <= 1024)),
    CONSTRAINT reports_client_version_check CHECK (((length(client_version) >= 1) AND (length(client_version) <= 256))),
    CONSTRAINT reports_id_check CHECK (public.is_uuid_v7(id)),
    CONSTRAINT reports_issue_id_check CHECK (public.is_uuid_v7(issue_id)),
    CONSTRAINT reports_kind_check CHECK ((kind = ANY (ARRAY['crash'::text, 'web_compat'::text]))),
    CONSTRAINT reports_linked_state_valid CHECK (((issue_id IS NULL) OR (state <> 'triage'::text))),
    CONSTRAINT reports_manifest_digest_check CHECK ((manifest_digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT reports_source_client_key_check CHECK (((source_client_key IS NULL) OR (source_client_key ~ '^[0-9a-f]{64}$'::text))),
    CONSTRAINT reports_staging_id_check CHECK (public.is_uuid_v7(staging_id)),
    CONSTRAINT reports_state_valid CHECK ((state = ANY (ARRAY['triage'::text, 'confirmed'::text, 'rejected'::text]))),
    CONSTRAINT reports_storage_state_check CHECK ((storage_state = ANY (ARRAY['pending_files'::text, 'ready'::text]))),
    CONSTRAINT reports_submission_id_check CHECK (public.is_uuid_v7(submission_id))
);


--
-- Name: runtime_configuration; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.runtime_configuration (
    singleton boolean DEFAULT true NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT runtime_configuration_singleton_check CHECK (singleton)
);


--
-- Name: sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sessions (
    token_hash text NOT NULL,
    github_id bigint NOT NULL,
    encrypted_access_token text NOT NULL,
    csrf_token text NOT NULL,
    membership_verified_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT sessions_token_hash_check CHECK ((length(token_hash) = 64))
);


--
-- Name: source_rate_limits; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.source_rate_limits (
    client_key text NOT NULL,
    source_report_id uuid,
    imposed_by bigint NOT NULL,
    imposed_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone,
    lifted_by bigint,
    lifted_at timestamp with time zone,
    CONSTRAINT source_rate_limits_check CHECK (((lifted_by IS NULL) = (lifted_at IS NULL))),
    CONSTRAINT source_rate_limits_client_key_check CHECK ((client_key ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT source_rate_limits_source_report_id_check CHECK (public.is_uuid_v7(source_report_id))
);


--
-- Name: upload_leases; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.upload_leases (
    id uuid NOT NULL,
    ip_key text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT upload_leases_id_check CHECK (public.is_uuid_v7(id))
);


--
-- Name: attachments attachments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.attachments
    ADD CONSTRAINT attachments_pkey PRIMARY KEY (id);


--
-- Name: attachments attachments_report_id_client_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.attachments
    ADD CONSTRAINT attachments_report_id_client_id_key UNIQUE (report_id, client_id);


--
-- Name: attachments attachments_storage_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.attachments
    ADD CONSTRAINT attachments_storage_key_key UNIQUE (storage_key);


--
-- Name: audit_events audit_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_events
    ADD CONSTRAINT audit_events_pkey PRIMARY KEY (id);


--
-- Name: bootstrap_credentials bootstrap_credentials_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bootstrap_credentials
    ADD CONSTRAINT bootstrap_credentials_pkey PRIMARY KEY (singleton);


--
-- Name: challenges challenges_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.challenges
    ADD CONSTRAINT challenges_pkey PRIMARY KEY (id);


--
-- Name: discord_delivery_state discord_delivery_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.discord_delivery_state
    ADD CONSTRAINT discord_delivery_state_pkey PRIMARY KEY (singleton);


--
-- Name: discord_report_notifications discord_report_notifications_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.discord_report_notifications
    ADD CONSTRAINT discord_report_notifications_pkey PRIMARY KEY (report_id);


--
-- Name: field_definitions field_definitions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.field_definitions
    ADD CONSTRAINT field_definitions_pkey PRIMARY KEY (key);


--
-- Name: issue_github_aliases issue_github_aliases_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.issue_github_aliases
    ADD CONSTRAINT issue_github_aliases_pkey PRIMARY KEY (github_repository, github_number);


--
-- Name: issues issues_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.issues
    ADD CONSTRAINT issues_pkey PRIMARY KEY (id);


--
-- Name: maintainers maintainers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.maintainers
    ADD CONSTRAINT maintainers_pkey PRIMARY KEY (github_id);


--
-- Name: oauth_states oauth_states_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_states
    ADD CONSTRAINT oauth_states_pkey PRIMARY KEY (state_hash);


--
-- Name: rate_buckets rate_buckets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rate_buckets
    ADD CONSTRAINT rate_buckets_pkey PRIMARY KEY (key);


--
-- Name: report_fields report_fields_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_fields
    ADD CONSTRAINT report_fields_pkey PRIMARY KEY (report_id, key);


--
-- Name: report_stack_signatures report_stack_signatures_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_stack_signatures
    ADD CONSTRAINT report_stack_signatures_pkey PRIMARY KEY (report_id, field_key);


--
-- Name: reports reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_pkey PRIMARY KEY (id);


--
-- Name: reports reports_staging_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_staging_id_key UNIQUE (staging_id);


--
-- Name: reports reports_submission_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_submission_id_key UNIQUE (submission_id);


--
-- Name: runtime_configuration runtime_configuration_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.runtime_configuration
    ADD CONSTRAINT runtime_configuration_pkey PRIMARY KEY (singleton);


--
-- Name: sessions sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_pkey PRIMARY KEY (token_hash);


--
-- Name: source_rate_limits source_rate_limits_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.source_rate_limits
    ADD CONSTRAINT source_rate_limits_pkey PRIMARY KEY (client_key);


--
-- Name: upload_leases upload_leases_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.upload_leases
    ADD CONSTRAINT upload_leases_pkey PRIMARY KEY (id);


--
-- Name: attachments_expiry; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX attachments_expiry ON public.attachments USING btree (expires_at) WHERE (expires_at IS NOT NULL);


--
-- Name: attachments_pending_failure_reason; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX attachments_pending_failure_reason ON public.attachments USING btree (created_at) WHERE ((failure_reason_processed_at IS NULL) AND (name = 'crash-diagnostics.txt'::text) AND (media_type = 'text/plain'::text));


--
-- Name: audit_events_entity; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX audit_events_entity ON public.audit_events USING btree (entity_id, id DESC);


--
-- Name: challenges_expiry; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX challenges_expiry ON public.challenges USING btree (expires_at);


--
-- Name: discord_report_notifications_pending; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX discord_report_notifications_pending ON public.discord_report_notifications USING btree (created_at, report_id) WHERE (delivered_at IS NULL);


--
-- Name: issue_github_aliases_by_issue; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX issue_github_aliases_by_issue ON public.issue_github_aliases USING btree (issue_id);


--
-- Name: issues_by_github_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX issues_by_github_id ON public.issues USING btree (github_issue_id) WHERE (github_issue_id IS NOT NULL);


--
-- Name: issues_visible; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX issues_visible ON public.issues USING btree (updated_at DESC) WHERE (state <> 'rejected'::text);


--
-- Name: oauth_states_expiry; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX oauth_states_expiry ON public.oauth_states USING btree (expires_at);


--
-- Name: one_active_issue_per_github_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX one_active_issue_per_github_id ON public.issues USING btree (github_issue_id) WHERE ((github_issue_id IS NOT NULL) AND (merged_into IS NULL) AND (state <> 'rejected'::text));


--
-- Name: one_active_issue_per_github_issue; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX one_active_issue_per_github_issue ON public.issues USING btree (lower(github_repository), github_number) WHERE ((merged_into IS NULL) AND (state <> 'rejected'::text));


--
-- Name: rate_buckets_expiry; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX rate_buckets_expiry ON public.rate_buckets USING btree (expires_at);


--
-- Name: report_fields_stack_index_candidates; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX report_fields_stack_index_candidates ON public.report_fields USING btree (report_id, key) WHERE ((kind = 'stack_trace'::text) OR ((key = 'stack'::text) AND (kind = 'multiline'::text)));


--
-- Name: report_stack_fingerprint; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX report_stack_fingerprint ON public.report_stack_signatures USING btree (fingerprint) WHERE (fingerprint IS NOT NULL);


--
-- Name: report_stack_frame_keys; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX report_stack_frame_keys ON public.report_stack_signatures USING gin (frame_keys);


--
-- Name: reports_by_issue; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reports_by_issue ON public.reports USING btree (issue_id);


--
-- Name: reports_expiry; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reports_expiry ON public.reports USING btree (expires_at) WHERE (expires_at IS NOT NULL);


--
-- Name: reports_pending_storage; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reports_pending_storage ON public.reports USING btree (created_at) WHERE (storage_state = 'pending_files'::text);


--
-- Name: reports_triage; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reports_triage ON public.reports USING btree (created_at DESC, id DESC) WHERE ((state = 'triage'::text) AND (storage_state = 'ready'::text));


--
-- Name: reports_visible; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reports_visible ON public.reports USING btree (created_at DESC, id DESC) WHERE ((state <> 'rejected'::text) AND (storage_state = 'ready'::text));


--
-- Name: sessions_expiry; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX sessions_expiry ON public.sessions USING btree (expires_at);


--
-- Name: upload_leases_expiry; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX upload_leases_expiry ON public.upload_leases USING btree (expires_at);


--
-- Name: reports enqueue_discord_report_after_insert; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER enqueue_discord_report_after_insert AFTER INSERT ON public.reports FOR EACH ROW EXECUTE FUNCTION public.enqueue_discord_report_notification();


--
-- Name: reports enqueue_discord_report_after_storage_ready; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER enqueue_discord_report_after_storage_ready AFTER UPDATE OF storage_state ON public.reports FOR EACH ROW WHEN (((old.storage_state <> 'ready'::text) AND (new.storage_state = 'ready'::text))) EXECUTE FUNCTION public.enqueue_discord_report_notification();


--
-- Name: runtime_configuration runtime_configuration_notify_change; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER runtime_configuration_notify_change AFTER UPDATE OF value ON public.runtime_configuration FOR EACH ROW WHEN ((old.value IS DISTINCT FROM new.value)) EXECUTE FUNCTION public.notify_runtime_configuration_change();


--
-- Name: attachments attachments_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.attachments
    ADD CONSTRAINT attachments_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: audit_events audit_events_actor_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_events
    ADD CONSTRAINT audit_events_actor_fkey FOREIGN KEY (actor) REFERENCES public.maintainers(github_id);


--
-- Name: challenges challenges_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.challenges
    ADD CONSTRAINT challenges_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id);


--
-- Name: discord_report_notifications discord_report_notifications_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.discord_report_notifications
    ADD CONSTRAINT discord_report_notifications_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: issue_github_aliases issue_github_aliases_issue_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.issue_github_aliases
    ADD CONSTRAINT issue_github_aliases_issue_id_fkey FOREIGN KEY (issue_id) REFERENCES public.issues(id);


--
-- Name: issues issues_merged_into_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.issues
    ADD CONSTRAINT issues_merged_into_fkey FOREIGN KEY (merged_into) REFERENCES public.issues(id);


--
-- Name: report_fields report_fields_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_fields
    ADD CONSTRAINT report_fields_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: report_stack_signatures report_stack_signatures_report_id_field_key_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_stack_signatures
    ADD CONSTRAINT report_stack_signatures_report_id_field_key_fkey FOREIGN KEY (report_id, field_key) REFERENCES public.report_fields(report_id, key) ON DELETE CASCADE;


--
-- Name: reports reports_issue_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_issue_id_fkey FOREIGN KEY (issue_id) REFERENCES public.issues(id);


--
-- Name: sessions sessions_github_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_github_id_fkey FOREIGN KEY (github_id) REFERENCES public.maintainers(github_id);


--
-- Name: source_rate_limits source_rate_limits_imposed_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.source_rate_limits
    ADD CONSTRAINT source_rate_limits_imposed_by_fkey FOREIGN KEY (imposed_by) REFERENCES public.maintainers(github_id);


--
-- Name: source_rate_limits source_rate_limits_lifted_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.source_rate_limits
    ADD CONSTRAINT source_rate_limits_lifted_by_fkey FOREIGN KEY (lifted_by) REFERENCES public.maintainers(github_id);


--
-- Name: source_rate_limits source_rate_limits_source_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.source_rate_limits
    ADD CONSTRAINT source_rate_limits_source_report_id_fkey FOREIGN KEY (source_report_id) REFERENCES public.reports(id) ON DELETE SET NULL;


--

-- Initial application data. Production retains its own configuration and field definitions.
INSERT INTO public.discord_delivery_state DEFAULT VALUES;
INSERT INTO public.runtime_configuration (value) VALUES ('{"limits": {"png_pixels": 25000000, "maximum_fields": 64, "metadata_bytes": 1048576, "challenge_burst": 3, "attachment_bytes": 10485760, "short_text_bytes": 4096, "submission_burst": 2, "submission_bytes": 26214400, "png_decoded_bytes": 33554432, "challenges_per_hour": 100, "maximum_attachments": 5, "multiline_text_bytes": 262144, "public_request_burst": 30, "submissions_per_hour": 50, "challenges_per_minute": 10, "submissions_per_minute": 5, "upload_timeout_seconds": 120, "concurrent_uploads_global": 16, "concurrent_uploads_per_ip": 2, "minimum_free_storage_bytes": 5368709120, "public_requests_per_minute": 120}, "discord": {"webhook_url": null, "stack_trace_lines": 20, "initial_retry_seconds": 30, "maximum_retry_seconds": 3600, "poll_interval_seconds": 2, "stack_trace_characters": 3000, "request_timeout_seconds": 10}, "maintenance": {"report_retention_days": 3650, "sweep_interval_seconds": 900, "staging_retention_seconds": 3600, "submission_source_retention_days": 30}, "proof_of_work": {"expected_work": 5244236, "challenge_lifetime_seconds": 600}, "admin_base_url": "http://localhost:3000", "public_base_url": "http://localhost:3001", "trusted_proxies": [], "github_repository": "LadybirdBrowser/ladybird", "github_authorization_team": "LadybirdBrowser/maintainers", "membership_recheck_seconds": 300, "github_reports_issue_field_id": null}'::jsonb);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('signal', 'Termination signal', 'text', 10);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('platform', 'Platform', 'text', 20);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('architecture', 'Architecture', 'text', 30);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('description', 'Description', 'multiline', 40);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('hostname', 'Website hostname', 'text', 50);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('url', 'Page URL', 'text', 60);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('process', 'Process', 'text', 35);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('signal_number', 'Signal number', 'number', 11);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('stack', 'Stack trace', 'stack_trace', 0);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('git_commit', 'Git commit', 'text', 32);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('build_configuration', 'Build configuration', 'text', 33);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('cpp_compiler', 'C++ compiler', 'text', 34);
INSERT INTO public.field_definitions (key, label, kind, "position") VALUES ('failure_reason', 'Failure reason', 'text', 9);
