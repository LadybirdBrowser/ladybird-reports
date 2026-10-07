-- Nothing can use these indexes. No query filters on `frame_keys`; the partial indexes
-- match neither the stack trace view nor the expiry queries.

DROP INDEX public.report_stack_frame_keys;
DROP INDEX public.report_fields_stack_index_candidates;
DROP INDEX public.attachments_expiry;

-- Nothing reads these columns either. A signature is its fingerprint, and a
-- stack without enough frames has none; `frame_keys` and `status` only repeated
-- what the stack text and the fingerprint already say, and `indexed_at` and
-- `github_checked_at` were only ever written.
ALTER TABLE public.report_stack_signatures
    DROP COLUMN status,
    DROP COLUMN frame_keys,
    DROP COLUMN indexed_at;

ALTER TABLE public.issues DROP COLUMN github_checked_at;

DROP FUNCTION public.accept_stack_signature(uuid, integer, text, text[]);

CREATE FUNCTION public.accept_stack_signature(report uuid, version integer, print text) RETURNS void
    LANGUAGE sql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
    INSERT INTO report_stack_signatures (report_id, algorithm_version, fingerprint)
    SELECT id, version, print
    FROM reports
    WHERE id = report AND storage_state = 'pending_files';
$$;
