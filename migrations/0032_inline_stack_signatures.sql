-- A report has one stack signature, stored while the report is accepted.
-- `matched_at` records that the admin service has matched it against issues: a
-- signature without it still waits for that, and a report with such a signature
-- is held back from Discord until then.

-- Of the stack traces a report has, `stack` comes first, then the first by key.
CREATE VIEW public.report_stack_traces AS
SELECT DISTINCT ON (fields.report_id)
    fields.report_id,
    fields.value #>> '{}' AS text
FROM public.report_fields AS fields
LEFT JOIN public.field_definitions AS definitions ON definitions.key = fields.key
WHERE fields.kind = 'stack_trace'
    OR (fields.kind = 'multiline'
        AND (fields.key = 'stack' OR definitions.kind = 'stack_trace'))
ORDER BY fields.report_id, (fields.key = 'stack') DESC, fields.key;

DELETE FROM public.report_stack_signatures
WHERE (report_id, field_key) NOT IN (
    SELECT DISTINCT ON (report_id) report_id, field_key
    FROM public.report_stack_signatures
    ORDER BY report_id, (field_key = 'stack') DESC, field_key
);

ALTER TABLE public.report_stack_signatures
    DROP CONSTRAINT report_stack_signatures_pkey,
    DROP COLUMN field_key,
    ADD COLUMN matched_at timestamp with time zone,
    ADD PRIMARY KEY (report_id),
    ADD FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;

UPDATE public.report_stack_signatures SET matched_at = indexed_at;

-- Called by the ingestion service in the transaction that accepts a report, so
-- it can only sign a report that is still being accepted.
CREATE FUNCTION public.accept_stack_signature(report uuid, version integer, print text, keys text[]) RETURNS void
    LANGUAGE sql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
    INSERT INTO report_stack_signatures (report_id, algorithm_version, status, fingerprint, frame_keys)
    SELECT id, version, CASE WHEN print IS NULL THEN 'insufficient' ELSE 'parsed' END, print, keys
    FROM reports
    WHERE id = report AND storage_state = 'pending_files';
$$;
