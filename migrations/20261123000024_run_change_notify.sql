-- Live run view: signal `huxley_run_changes` with the run id whenever a run
-- or one of its steps changes. Postgres folds identical signals within a
-- transaction into one, delivered at commit.
CREATE FUNCTION huxley_notify_run_change() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('huxley_run_changes', NEW.id::text);
    RETURN NULL;
END;
$$;

CREATE FUNCTION huxley_notify_step_run_change() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('huxley_run_changes', NEW.run_id::text);
    RETURN NULL;
END;
$$;

CREATE TRIGGER runs_notify_change
    AFTER INSERT OR UPDATE ON runs
    FOR EACH ROW EXECUTE FUNCTION huxley_notify_run_change();

CREATE TRIGGER step_runs_notify_change
    AFTER INSERT OR UPDATE ON step_runs
    FOR EACH ROW EXECUTE FUNCTION huxley_notify_step_run_change();
