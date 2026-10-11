-- A report that is tracked by an issue follows it: confirmed while the issue is
-- open, resolved once the issue is. The reports of an issue change with it, and
-- a report that is linked to an issue, or confirmed again, takes the state its
-- issue calls for.

ALTER TABLE public.reports DROP CONSTRAINT reports_state_valid;
ALTER TABLE public.reports ADD CONSTRAINT reports_state_valid
    CHECK (state = ANY (ARRAY['triage', 'confirmed', 'resolved', 'rejected']));

CREATE FUNCTION public.follow_issue_state_on_report() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'public', 'pg_temp'
    AS $$
BEGIN
    NEW.state := CASE
        WHEN (SELECT state FROM issues WHERE id = NEW.issue_id) = 'resolved' THEN 'resolved'
        ELSE 'confirmed'
    END;

    RETURN NEW;
END;
$$;

CREATE TRIGGER follow_issue_state
    BEFORE INSERT OR UPDATE OF state, issue_id ON public.reports
    FOR EACH ROW
    WHEN (NEW.issue_id IS NOT NULL AND NEW.state IN ('confirmed', 'resolved'))
    EXECUTE FUNCTION public.follow_issue_state_on_report();

CREATE FUNCTION public.follow_issue_state_on_issue() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'public', 'pg_temp'
    AS $$
BEGIN
    IF NEW.state = 'resolved' THEN
        WITH changed AS (
            UPDATE reports SET state = 'resolved', updated_at = now()
            WHERE issue_id = NEW.id AND state = 'confirmed'
            RETURNING id
        )
        INSERT INTO audit_events (action, entity_id, details)
        SELECT 'report.update_state', id,
            jsonb_build_object('from', 'confirmed', 'to', 'resolved', 'source', 'issue')
        FROM changed;
    ELSE
        WITH changed AS (
            UPDATE reports SET state = 'confirmed', updated_at = now()
            WHERE issue_id = NEW.id AND state = 'resolved'
            RETURNING id
        )
        INSERT INTO audit_events (action, entity_id, details)
        SELECT 'report.update_state', id,
            jsonb_build_object('from', 'resolved', 'to', 'confirmed', 'source', 'issue')
        FROM changed;
    END IF;

    RETURN NULL;
END;
$$;

CREATE TRIGGER follow_issue_state
    AFTER UPDATE OF state ON public.issues
    FOR EACH ROW
    WHEN (OLD.state IS DISTINCT FROM NEW.state AND (NEW.state = 'resolved' OR OLD.state = 'resolved'))
    EXECUTE FUNCTION public.follow_issue_state_on_issue();

-- The reports of issues that are resolved already.
WITH changed AS (
    UPDATE public.reports SET state = 'resolved', updated_at = now()
    FROM public.issues
    WHERE issues.id = reports.issue_id AND issues.state = 'resolved' AND reports.state = 'confirmed'
    RETURNING reports.id
)
INSERT INTO public.audit_events (action, entity_id, details)
SELECT 'report.update_state', id,
    jsonb_build_object('from', 'confirmed', 'to', 'resolved', 'source', 'issue')
FROM changed;
