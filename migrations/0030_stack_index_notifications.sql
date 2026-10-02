-- Stack traces are indexed by the admin service. Instead of polling for work, it
-- listens on this channel. A payload is the id of a report that became ready;
-- an empty payload means any report may need another look.

CREATE FUNCTION public.notify_report_ready_for_stack_index() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'public', 'pg_temp'
    AS $$
BEGIN
    PERFORM pg_notify('ladybird_reports_stack_index', NEW.id::text);

    RETURN NEW;
END;
$$;

CREATE FUNCTION public.notify_field_definition_for_stack_index() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'public', 'pg_temp'
    AS $$
BEGIN
    PERFORM pg_notify('ladybird_reports_stack_index', '');

    RETURN NEW;
END;
$$;

-- Fields are written while a report is still pending its files, so readiness is
-- the moment a report's stack traces become eligible. These mirror the triggers
-- that queue Discord notifications.
CREATE TRIGGER notify_stack_index_after_insert
    AFTER INSERT ON public.reports
    FOR EACH ROW WHEN (new.storage_state = 'ready')
    EXECUTE FUNCTION public.notify_report_ready_for_stack_index();

CREATE TRIGGER notify_stack_index_after_storage_ready
    AFTER UPDATE OF storage_state ON public.reports
    FOR EACH ROW WHEN (old.storage_state <> 'ready' AND new.storage_state = 'ready')
    EXECUTE FUNCTION public.notify_report_ready_for_stack_index();

-- Defining a field as a stack trace turns existing multiline fields into stack
-- traces that have no signature yet.
CREATE TRIGGER notify_stack_index_after_field_definition
    AFTER INSERT OR UPDATE OF kind ON public.field_definitions
    FOR EACH ROW
    EXECUTE FUNCTION public.notify_field_definition_for_stack_index();
