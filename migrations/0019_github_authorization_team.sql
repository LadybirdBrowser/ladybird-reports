UPDATE runtime_configuration
SET value = jsonb_set(
    value,
    '{github_authorization_team}',
    '"LadybirdBrowser/maintainers"'::jsonb
)
WHERE singleton = true
    AND NOT value ? 'github_authorization_team';
