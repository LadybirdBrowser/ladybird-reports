ALTER TABLE sessions
    ADD COLUMN encrypted_refresh_token text,
    ADD COLUMN access_token_expires_at timestamptz,
    ADD COLUMN refresh_token_expires_at timestamptz;

UPDATE runtime_configuration
SET value = value || '{
    "membership_recheck_seconds": 600,
    "session_lifetime_seconds": 86400,
    "token_refresh_before_seconds": 900
}'::jsonb
WHERE singleton = true;
