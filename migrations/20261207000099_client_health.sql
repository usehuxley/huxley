-- Client health (design doc §8.9): a daily score per client company from
-- weighted factors (satisfaction, NPS, SLA misses, reopened tickets, ticket
-- volume, renewals), with the reasons behind it.

-- The weights an organization's clients are scored with (the nearest up
-- the tree applies; none: the defaults).
CREATE TABLE cx_health_models (
    org_id      uuid PRIMARY KEY REFERENCES organizations (id) ON DELETE CASCADE,
    weights     jsonb NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE cx_health_models ENABLE ROW LEVEL SECURITY;
CREATE POLICY cx_health_models_all ON cx_health_models FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- A company's score on a day.
CREATE TABLE cx_health (
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    computed_on  date NOT NULL,
    score        smallint CHECK (score BETWEEN 0 AND 100),
    -- [{ key, label, score, detail }]
    factors      jsonb NOT NULL,
    -- The weakest factors' details, worst first.
    reasons      jsonb NOT NULL,
    PRIMARY KEY (org_id, computed_on)
);
ALTER TABLE cx_health ENABLE ROW LEVEL SECURITY;
CREATE POLICY cx_health_all ON cx_health FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
