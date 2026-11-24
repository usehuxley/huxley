-- lookup() in expressions: each run records a lookup's answer the first time
-- it asks, so replays (every resume re-evaluates expressions) see the same
-- row and take the same path. Dataset rows are client data, so these follow
-- the run's organization like step_runs.
CREATE TABLE run_lookups (
    run_id      uuid NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- The request, canonical: {"dataset": …, "filters": {…}}.
    key         text NOT NULL,
    -- The matching row (null: no match).
    row         jsonb,
    -- Why it failed (an unknown dataset, say); a resumed run asks again.
    error       text,
    fetched_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, key)
);

ALTER TABLE run_lookups ENABLE ROW LEVEL SECURITY;
CREATE POLICY run_lookups_all ON run_lookups FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
