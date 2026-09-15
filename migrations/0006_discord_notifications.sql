UPDATE runtime_configuration
SET value = jsonb_set(
    value,
    '{discord}',
    '{
      "webhook_url": null,
      "poll_interval_seconds": 2,
      "request_timeout_seconds": 10,
      "initial_retry_seconds": 30,
      "maximum_retry_seconds": 3600,
      "stack_trace_lines": 20,
      "stack_trace_characters": 3000
    }'::jsonb,
    true
);

CREATE FUNCTION reporting_runtime_configuration()
RETURNS jsonb
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = public, pg_temp
AS $$
    SELECT value - 'discord'
    FROM runtime_configuration
    WHERE singleton = true;
$$;

REVOKE ALL ON FUNCTION reporting_runtime_configuration() FROM PUBLIC;

CREATE TABLE discord_report_notifications (
    report_id uuid PRIMARY KEY
        REFERENCES reports ON DELETE CASCADE
        CHECK (is_uuid_v7(report_id)),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    last_failure text CHECK (last_failure IS NULL OR length(last_failure) <= 128),
    last_response_status smallint CHECK (
        last_response_status IS NULL
        OR last_response_status BETWEEN 100 AND 599
    ),
    created_at timestamptz NOT NULL DEFAULT now(),
    delivered_at timestamptz
);

CREATE INDEX discord_report_notifications_pending
    ON discord_report_notifications (created_at, report_id)
    WHERE delivered_at IS NULL;

CREATE TABLE discord_delivery_state (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    lease_id uuid CHECK (is_uuid_v7(lease_id)),
    leased_report_id uuid CHECK (is_uuid_v7(leased_report_id)),
    lease_expires_at timestamptz,
    paused_until timestamptz,
    consecutive_failures integer NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    CHECK (
        (
            lease_id IS NULL
            AND leased_report_id IS NULL
            AND lease_expires_at IS NULL
        )
        OR (
            lease_id IS NOT NULL
            AND leased_report_id IS NOT NULL
            AND lease_expires_at IS NOT NULL
        )
    )
);

INSERT INTO discord_delivery_state DEFAULT VALUES;

CREATE FUNCTION enqueue_discord_report_notification()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_temp
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

REVOKE ALL ON FUNCTION enqueue_discord_report_notification() FROM PUBLIC;

CREATE TRIGGER enqueue_discord_report_after_insert
AFTER INSERT ON reports
FOR EACH ROW
EXECUTE FUNCTION enqueue_discord_report_notification();

CREATE TRIGGER enqueue_discord_report_after_storage_ready
AFTER UPDATE OF storage_state ON reports
FOR EACH ROW
WHEN (OLD.storage_state <> 'ready' AND NEW.storage_state = 'ready')
EXECUTE FUNCTION enqueue_discord_report_notification();
