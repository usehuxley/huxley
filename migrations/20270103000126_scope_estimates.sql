-- Presales estimates (design doc §8.10, ADR-073): a scope's Work Breakdown
-- Structure (phases and tasks, hours by role, low / likely / high) and Bill
-- of Materials (catalog lines sized from the client's environment). AI
-- drafts both from the Scope of Work, grounded in project templates and
-- the hours logged on past projects' tasks; people edit them. Contingency
-- follows the scope's risk. Together they assemble the quote.

ALTER TABLE sales_scopes
    ADD COLUMN risk text NOT NULL DEFAULT 'medium' CHECK (risk IN ('low', 'medium', 'high')),
    ADD COLUMN estimate_state text NOT NULL DEFAULT 'idle' CHECK (estimate_state IN ('idle', 'requested', 'failed')),
    ADD COLUMN estimate_error text CHECK (length(estimate_error) <= 2000),
    ADD COLUMN estimate_requested_at timestamptz,
    ADD COLUMN estimated_at timestamptz;

CREATE TABLE sales_scope_tasks (
    id            uuid PRIMARY KEY,
    scope_id      uuid NOT NULL REFERENCES sales_scopes (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    position      integer NOT NULL,
    phase         text NOT NULL CHECK (length(btrim(phase)) BETWEEN 1 AND 100),
    name          text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 300),
    -- Who does it: a labour service from the catalog (priced per hour).
    role_product_id uuid REFERENCES catalog_products (id) ON DELETE SET NULL,
    role          text CHECK (length(role) <= 100),
    hours_low     numeric(10,2) NOT NULL DEFAULT 0 CHECK (hours_low >= 0),
    hours_likely  numeric(10,2) NOT NULL DEFAULT 0 CHECK (hours_likely >= hours_low),
    hours_high    numeric(10,2) NOT NULL DEFAULT 0 CHECK (hours_high >= hours_likely),
    milestone     boolean NOT NULL DEFAULT false,
    -- The Scope of Work deliverable it serves.
    deliverable   text CHECK (length(deliverable) <= 300),
    drafted       boolean NOT NULL DEFAULT false
);
CREATE INDEX sales_scope_tasks_scope ON sales_scope_tasks (scope_id, position);

CREATE TABLE sales_scope_items (
    id          uuid PRIMARY KEY,
    scope_id    uuid NOT NULL REFERENCES sales_scopes (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    position    integer NOT NULL,
    product_id  uuid REFERENCES catalog_products (id) ON DELETE SET NULL,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 300),
    quantity    numeric(18,4) NOT NULL CHECK (quantity > 0),
    -- How the quantity was arrived at: "one per user (30 users)".
    basis       text CHECK (length(basis) <= 300),
    -- An alternative the client may choose instead (an option on the quote).
    alternate   boolean NOT NULL DEFAULT false,
    drafted     boolean NOT NULL DEFAULT false
);
CREATE INDEX sales_scope_items_scope ON sales_scope_items (scope_id, position);

ALTER TABLE sales_scope_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales_scope_items ENABLE ROW LEVEL SECURITY;
CREATE POLICY sales_scope_tasks_all ON sales_scope_tasks FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
CREATE POLICY sales_scope_items_all ON sales_scope_items FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
