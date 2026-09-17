-- Report-to-issue changes are already timestamped in audit_events.
ALTER TABLE reports DROP COLUMN assigned_at;
