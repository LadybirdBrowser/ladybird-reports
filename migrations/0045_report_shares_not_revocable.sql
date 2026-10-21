-- A link that shares a report is not revoked: it expires after 24 hours.

ALTER TABLE public.report_shares DROP COLUMN revoked_at;
