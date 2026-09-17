CREATE FUNCTION notify_runtime_configuration_change()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM pg_notify('ladybird_reports_configuration', '');
    RETURN NEW;
END;
$$;

CREATE TRIGGER runtime_configuration_notify_change
AFTER UPDATE OF value ON runtime_configuration
FOR EACH ROW
WHEN (OLD.value IS DISTINCT FROM NEW.value)
EXECUTE FUNCTION notify_runtime_configuration_change();
