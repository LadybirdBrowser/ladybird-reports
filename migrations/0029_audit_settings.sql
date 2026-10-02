UPDATE runtime_configuration
SET value = value || '{"audit": {"denied_sign_in_window_seconds": 600}}'::jsonb
WHERE singleton = true;

-- The ingestion role never needs the administrative audit settings. Hiding
-- them, as with the Discord settings, also keeps a public API process that has
-- not been restarted onto this build yet from rejecting a key it does not know.
CREATE OR REPLACE FUNCTION public.reporting_runtime_configuration() RETURNS jsonb
    LANGUAGE sql STABLE SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
    SELECT value - 'discord' - 'audit'
    FROM runtime_configuration
    WHERE singleton = true;
$$;
