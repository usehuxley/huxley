-- Dashboards (design doc §8.12, ADR-041): a grid of widgets, each a query
-- spec and a visualization. Shared dashboards belong to an organization
-- (for it and those below); personal ones to their owner. Prebuilt ones are
-- defined in code.

CREATE TABLE dashboards (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- Set for a personal dashboard: only its owner sees it.
    owner_user_id  uuid REFERENCES users (id) ON DELETE CASCADE,
    name           text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    description    text CHECK (description IS NULL OR length(description) <= 500),
    -- The range widgets show unless they fix their own, e.g. last_30_days.
    range          text NOT NULL DEFAULT 'last_30_days',
    widgets        jsonb NOT NULL DEFAULT '[]',
    version        integer NOT NULL DEFAULT 1,
    created_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX dashboards_org ON dashboards (org_id);
CREATE INDEX dashboards_owner ON dashboards (owner_user_id) WHERE owner_user_id IS NOT NULL;
ALTER TABLE dashboards ENABLE ROW LEVEL SECURITY;
CREATE POLICY dashboards_all ON dashboards FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Technicians and read-only users see dashboards.
UPDATE roles SET permissions = permissions || '{dashboard:read}'
 WHERE key IN ('technician', 'read_only') AND is_system;
