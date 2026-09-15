UPDATE runtime_configuration
SET value = jsonb_set(
    jsonb_set(
        value,
        '{limits,png_decoded_bytes}',
        to_jsonb(33554432),
        true
    ),
    '{maintenance}',
    '{
      "sweep_interval_seconds": 900,
      "staging_retention_seconds": 3600,
      "report_retention_days": 3650
    }'::jsonb,
    true
);

CREATE INDEX reports_expiry ON reports (expires_at) WHERE deleted_at IS NULL;
CREATE INDEX reports_deletion ON reports (deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX attachments_expiry ON attachments (expires_at) WHERE deleted_at IS NULL;

CREATE FUNCTION configure_report_retention(report uuid, retention_days integer)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
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

CREATE FUNCTION sweep_expired_ingestion_state()
RETURNS TABLE (challenges_deleted bigint, rate_buckets_deleted bigint, upload_leases_deleted bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
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

REVOKE ALL ON FUNCTION configure_report_retention(uuid, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION sweep_expired_ingestion_state() FROM PUBLIC;
