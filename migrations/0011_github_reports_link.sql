ALTER TABLE issues
    ADD COLUMN github_reports_field_id bigint,
    ADD COLUMN github_reports_link_url text;

UPDATE runtime_configuration
SET value = jsonb_set(value, '{github_reports_issue_field_id}', 'null'::jsonb)
WHERE singleton = true
    AND NOT value ? 'github_reports_issue_field_id';
