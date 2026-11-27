-- Expenses (design doc §8.16, ADR-079): what people spend, and mileage,
-- against an in-progress ticket, a project (or one of its phases), a
-- client, or internally; with a receipt, tax, and whether it's billable
-- (marked up) and reimbursable. Reports and approval, billing and
-- reimbursement follow in later slices.

CREATE TABLE exp_categories (
    id                    uuid PRIMARY KEY,
    name                  text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    -- Mileage categories take a distance at a rate instead of an amount.
    kind                  text NOT NULL DEFAULT 'expense' CHECK (kind IN ('expense', 'mileage')),
    billable              boolean NOT NULL DEFAULT false,
    reimbursable          boolean NOT NULL DEFAULT true,
    markup_pct            numeric(9,4) NOT NULL DEFAULT 0 CHECK (markup_pct >= 0),
    -- Over this, an entry is flagged (policy).
    limit_amount          numeric(18,4) CHECK (limit_amount >= 0),
    receipt_required      boolean NOT NULL DEFAULT true,
    position              integer NOT NULL DEFAULT 0,
    active                boolean NOT NULL DEFAULT true,
    created_at            timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX exp_categories_name ON exp_categories (lower(name));

INSERT INTO exp_categories (id, name, kind, billable, reimbursable, receipt_required, position) VALUES
    ('01970000-0000-7000-8000-00000000e001', 'Mileage', 'mileage', true, true, false, 1),
    ('01970000-0000-7000-8000-00000000e002', 'Parts and supplies', 'expense', true, true, true, 2),
    ('01970000-0000-7000-8000-00000000e003', 'Travel and lodging', 'expense', true, true, true, 3),
    ('01970000-0000-7000-8000-00000000e004', 'Meals', 'expense', false, true, true, 4),
    ('01970000-0000-7000-8000-00000000e005', 'Software and subscriptions', 'expense', false, true, true, 5),
    ('01970000-0000-7000-8000-00000000e006', 'Other', 'expense', false, true, true, 6);

-- Mileage rates; the one in effect on the day is used.
CREATE TABLE exp_mileage_rates (
    id              uuid PRIMARY KEY,
    unit            text NOT NULL DEFAULT 'mi' CHECK (unit IN ('mi', 'km')),
    rate            numeric(18,4) NOT NULL CHECK (rate >= 0),
    currency        text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    effective_from  date NOT NULL,
    UNIQUE (unit, currency, effective_from)
);
INSERT INTO exp_mileage_rates (id, unit, rate, currency, effective_from)
VALUES ('01970000-0000-7000-8000-00000000e101', 'mi', 0.70, 'USD', '2025-01-01');

CREATE TABLE exp_entries (
    id             uuid PRIMARY KEY,
    -- Whose cost it is: a client (ticket, project or the client itself), or
    -- the provider (internal).
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id        uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    category_id    uuid NOT NULL REFERENCES exp_categories (id),
    spent_on       date NOT NULL,
    description    text NOT NULL CHECK (length(btrim(description)) BETWEEN 1 AND 500),
    vendor         text CHECK (length(vendor) <= 200),
    -- What was paid, tax included (mileage: distance × rate).
    amount         numeric(18,4) NOT NULL CHECK (amount >= 0),
    tax            numeric(18,4) NOT NULL DEFAULT 0 CHECK (tax >= 0),
    currency       text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    -- Mileage: the distance, its unit and the rate used.
    distance       numeric(18,4) CHECK (distance > 0),
    unit           text CHECK (unit IN ('mi', 'km')),
    rate           numeric(18,4),
    billable       boolean NOT NULL DEFAULT false,
    markup_pct     numeric(9,4) NOT NULL DEFAULT 0 CHECK (markup_pct >= 0),
    reimbursable   boolean NOT NULL DEFAULT true,
    ticket_id      uuid REFERENCES sd_tickets (id) ON DELETE SET NULL,
    project_id     uuid REFERENCES prj_projects (id) ON DELETE SET NULL,
    phase_id       uuid REFERENCES prj_phases (id) ON DELETE SET NULL,
    receipt_key    text,
    receipt_name   text CHECK (length(receipt_name) <= 255),
    receipt_type   text CHECK (length(receipt_type) <= 100),
    receipt_size   bigint,
    status         text NOT NULL DEFAULT 'draft'
                     CHECK (status IN ('draft', 'submitted', 'approved', 'rejected', 'reimbursed')),
    version        integer NOT NULL DEFAULT 1,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX exp_entries_user ON exp_entries (user_id, spent_on DESC);
CREATE INDEX exp_entries_ticket ON exp_entries (ticket_id) WHERE ticket_id IS NOT NULL;
CREATE INDEX exp_entries_project ON exp_entries (project_id) WHERE project_id IS NOT NULL;

ALTER TABLE exp_entries ENABLE ROW LEVEL SECURITY;
CREATE POLICY exp_entries_all ON exp_entries FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Technicians log their own expenses.
UPDATE roles SET permissions = permissions || '{expense:write}'
 WHERE key IN ('technician') AND is_system;
