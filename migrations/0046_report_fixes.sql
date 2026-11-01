-- A report that isn't linked to an issue can be resolved by the fix that resolved it: a merged pull request or a
-- commit on the repository's default branch. The report keeps the fix's reference, title and URL while it's resolved
-- and unlinked, and loses them when it leaves that state or is linked to an issue.

ALTER TABLE public.reports
    ADD COLUMN fix_kind text,
    ADD COLUMN fix_reference text,
    ADD COLUMN fix_title text,
    ADD COLUMN fix_url text;

ALTER TABLE public.reports ADD CONSTRAINT reports_fix_kind_valid
    CHECK (fix_kind IS NULL OR fix_kind = ANY (ARRAY['pull_request', 'commit']));
ALTER TABLE public.reports ADD CONSTRAINT reports_fix_complete
    CHECK ((fix_kind IS NULL) = (fix_reference IS NULL)
        AND (fix_kind IS NULL) = (fix_title IS NULL)
        AND (fix_kind IS NULL) = (fix_url IS NULL));
ALTER TABLE public.reports ADD CONSTRAINT reports_fix_lengths
    CHECK (length(fix_reference) <= 64 AND length(fix_title) <= 512 AND length(fix_url) <= 2048);
ALTER TABLE public.reports ADD CONSTRAINT reports_fix_resolves_unlinked
    CHECK (fix_kind IS NULL OR (state = 'resolved' AND issue_id IS NULL));

CREATE FUNCTION public.drop_fix_of_unresolved_report() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'public', 'pg_temp'
    AS $$
BEGIN
    NEW.fix_kind := NULL;
    NEW.fix_reference := NULL;
    NEW.fix_title := NULL;
    NEW.fix_url := NULL;
    RETURN NEW;
END;
$$;

-- Runs after follow_issue_state, which settles the state of a linked report, as triggers fire in name order.
CREATE TRIGGER keep_fix_with_resolution
    BEFORE UPDATE OF state, issue_id ON public.reports
    FOR EACH ROW
    WHEN (NEW.fix_kind IS NOT NULL AND (NEW.state <> 'resolved' OR NEW.issue_id IS NOT NULL))
    EXECUTE FUNCTION public.drop_fix_of_unresolved_report();
