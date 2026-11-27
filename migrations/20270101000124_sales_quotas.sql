-- Sales quotas (design doc §8.10, ADR-071): how much each salesperson is
-- expected to win in a month or a quarter, set by sales configurers. The
-- forecast compares quota with won deals and the open pipeline by
-- forecast category.

CREATE TABLE sales_quotas (
    id            uuid PRIMARY KEY,
    -- The organization the salesperson sells for (usually the provider).
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id       uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- The first day of the month or quarter.
    period_start  date NOT NULL CHECK (extract(day FROM period_start) = 1),
    months        integer NOT NULL CHECK (months IN (1, 3)),
    -- First-year value (one-time plus twelve months of recurring).
    amount        numeric(18,4) NOT NULL CHECK (amount >= 0),
    currency      text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, period_start, months)
);

ALTER TABLE sales_quotas ENABLE ROW LEVEL SECURITY;
CREATE POLICY sales_quotas_all ON sales_quotas FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
