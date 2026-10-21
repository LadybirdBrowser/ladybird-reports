-- Issues created from a report get the labels of its kind of report. The labels that already exist in the
-- repository are the initial ones; settings that someone has changed in the meantime stay as they are.

UPDATE public.runtime_configuration
SET value = jsonb_set(
    value,
    '{github,labels}',
    '{"crash": ["crash"], "web_compat": ["web compatibility"]}'::jsonb
)
WHERE singleton = true AND NOT (value -> 'github' ? 'labels');
