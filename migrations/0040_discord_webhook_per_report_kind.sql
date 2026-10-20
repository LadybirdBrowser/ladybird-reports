-- Crash reports and web compatibility reports are announced in separate Discord channels, each with its own
-- webhook. The webhook that announced every report becomes the one of both kinds, so nothing stops being announced
-- until a separate webhook is set for one of them.

UPDATE public.runtime_configuration
SET value = jsonb_set(
    value,
    '{discord}',
    (value -> 'discord') - 'webhook_url' || jsonb_build_object(
        'crash_webhook_url', value -> 'discord' -> 'webhook_url',
        'web_compat_webhook_url', value -> 'discord' -> 'webhook_url'
    )
)
WHERE singleton = true AND value -> 'discord' ? 'webhook_url';
