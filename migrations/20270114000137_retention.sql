-- Data retention (design doc §13, ADR-085): how long finished workflow runs
-- (with their spilled payloads), published domain events, read
-- notifications and AI call records are kept. A daily sweep removes what's
-- older. The audit log is kept: it's a hash chain, exported, not pruned.

CREATE TABLE retention_settings (
    id                 boolean PRIMARY KEY DEFAULT true CHECK (id),
    run_months         integer NOT NULL DEFAULT 12 CHECK (run_months BETWEEN 1 AND 120),
    event_days         integer NOT NULL DEFAULT 30 CHECK (event_days BETWEEN 1 AND 3650),
    notification_days  integer NOT NULL DEFAULT 180 CHECK (notification_days BETWEEN 1 AND 3650),
    ai_call_days       integer NOT NULL DEFAULT 365 CHECK (ai_call_days BETWEEN 1 AND 3650),
    last_swept_at      timestamptz,
    -- What the last sweep removed.
    last_sweep         jsonb,
    updated_at         timestamptz NOT NULL DEFAULT now()
);
INSERT INTO retention_settings (id) VALUES (true);

-- Drops run partitions (and their step partitions) wholly before `cutoff`
-- that are empty. Returns how many it dropped.
CREATE FUNCTION huxley_drop_empty_run_partitions(cutoff uuid) RETURNS integer
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp AS $$
DECLARE
    part record;
    dropped integer := 0;
    empty boolean;
BEGIN
    FOR part IN
        SELECT c.relname, pg_get_expr(c.relpartbound, c.oid) AS bound
          FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid JOIN pg_class p ON p.oid = i.inhparent
         WHERE p.relname = 'runs' AND c.relname ~ '^runs_[0-9]{4}_[0-9]{2}$'
    LOOP
        -- FOR VALUES FROM ('…') TO ('…'): the partition ends before the cutoff.
        IF substring(part.bound FROM 'TO \(''([0-9a-f-]+)''\)')::uuid <= cutoff THEN
            EXECUTE format('SELECT NOT EXISTS (SELECT 1 FROM %I)', part.relname) INTO empty;
            IF empty THEN
                EXECUTE format('DROP TABLE IF EXISTS %I', 'step_' || part.relname);
                EXECUTE format('DROP TABLE %I', part.relname);
                dropped := dropped + 1;
            END IF;
        END IF;
    END LOOP;
    RETURN dropped;
END $$;

GRANT EXECUTE ON FUNCTION huxley_drop_empty_run_partitions(uuid) TO huxley_app;

-- The sweep (in system scope) removes published events; nothing else may.
CREATE POLICY events_outbox_delete ON events_outbox FOR DELETE TO huxley_app
    USING ((SELECT huxley_scope_all()) AND published_at IS NOT NULL);
