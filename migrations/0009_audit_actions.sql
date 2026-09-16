UPDATE audit_events
SET
    action = 'report.update_state',
    details = details || '{"from": "triage", "to": "confirmed"}'::jsonb
WHERE action = 'report.confirmed';

UPDATE audit_events
SET
    action = 'report.update_state',
    details = details || '{"from": "confirmed", "to": "triage"}'::jsonb
WHERE action = 'report.returned_to_triage';

UPDATE audit_events
SET action = 'report.update_issue'
WHERE action IN ('report.assignment', 'report.assigned');

UPDATE audit_events
SET
    action = 'report.update_visibility',
    details = details || '{"from": "visible", "to": "hidden"}'::jsonb
WHERE action = 'report.hidden';

UPDATE audit_events
SET
    action = 'submission_source.update_state',
    details = jsonb_build_object(
        'from', 'allowed',
        'to', 'blocked',
        'triage_reports_hidden', COALESCE(
            details->'triage_reports_removed',
            '0'::jsonb
        )
    )
WHERE action = 'report.source_blocked';

UPDATE audit_events
SET
    action = 'submission_source.update_state',
    details = details || '{"from": "blocked", "to": "allowed"}'::jsonb
WHERE action = 'report.source_unblocked';

UPDATE audit_events SET action = 'issue.create'
WHERE action = 'issue.created';

UPDATE audit_events SET action = 'issue.update'
WHERE action = 'issue.updated';

UPDATE audit_events SET action = 'issue.merge'
WHERE action = 'issue.merged';

UPDATE audit_events SET action = 'issue.update_github_link'
WHERE action IN ('issue.github_linked', 'issue.github_published');

UPDATE audit_events SET action = 'configuration.update'
WHERE action = 'configuration.updated';

UPDATE audit_events SET action = 'field_definition.update'
WHERE action = 'field_definition.updated';

UPDATE audit_events SET action = 'field_definitions.reorder'
WHERE action = 'field_definitions.reordered';
