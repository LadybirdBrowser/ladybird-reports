-- The settings of GitHub are grouped under `github`, with the prefix of their names dropped, and the webhooks of
-- Discord under `discord.webhook_urls`, by kind of report. The values stay as they are.

UPDATE public.runtime_configuration
SET value = jsonb_set(
    (value
        - 'github_repository'
        - 'github_authorization_team'
        - 'github_reports_issue_field_id'
        - 'token_refresh_before_seconds'
        - 'membership_recheck_seconds')
    || jsonb_build_object('github', jsonb_build_object(
        'repository', COALESCE(value -> 'github_repository', '"LadybirdBrowser/ladybird"'::jsonb),
        'authorization_team', COALESCE(value -> 'github_authorization_team', '"LadybirdBrowser/maintainers"'::jsonb),
        'reports_issue_field_id', COALESCE(value -> 'github_reports_issue_field_id', 'null'::jsonb),
        'token_refresh_before_seconds', COALESCE(value -> 'token_refresh_before_seconds', '900'::jsonb),
        'membership_recheck_seconds', COALESCE(value -> 'membership_recheck_seconds', '1800'::jsonb)
    )),
    '{discord}',
    ((value -> 'discord') - 'crash_webhook_url' - 'web_compat_webhook_url') || jsonb_build_object(
        'webhook_urls', jsonb_build_object(
            'crash', COALESCE(value #> '{discord,crash_webhook_url}', 'null'::jsonb),
            'web_compat', COALESCE(value #> '{discord,web_compat_webhook_url}', 'null'::jsonb)
        )
    )
)
WHERE singleton = true AND value ? 'github_repository';
