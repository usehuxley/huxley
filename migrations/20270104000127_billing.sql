-- Invoicing (design doc §8.11, ADR-075): invoices generated from what
-- Huxley tracks (billable time after agreement coverage, delivered sales
-- order lines, agreements' recurring charges with proration, project fees
-- and milestones), in billing runs per period. Every billed source is
-- marked once, so nothing is billed twice; voiding gives them back.
-- Credit memos correct invoices that have gone out.

-- Per provider: the fallback hourly rate (time no agreement prices), the
-- currency and payment terms.
CREATE TABLE billing_settings (
    org_id          uuid PRIMARY KEY REFERENCES organizations (id) ON DELETE CASCADE,
    hourly_rate     numeric(18,4) CHECK (hourly_rate >= 0),
    currency        text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    payment_days    integer NOT NULL DEFAULT 30 CHECK (payment_days BETWEEN 0 AND 365),
    -- Only time on approved timesheets is billed.
    approved_time_only boolean NOT NULL DEFAULT false
);

CREATE TABLE billing_runs (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    period_start  date NOT NULL,
    period_end    date NOT NULL CHECK (period_end >= period_start),
    invoices      integer NOT NULL DEFAULT 0,
    created_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE SEQUENCE billing_invoice_number;

CREATE TABLE billing_invoices (
    id            uuid PRIMARY KEY,
    -- The client billed.
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    number        bigint NOT NULL UNIQUE DEFAULT nextval('billing_invoice_number'),
    kind          text NOT NULL DEFAULT 'invoice' CHECK (kind IN ('invoice', 'credit_memo')),
    -- A credit memo's invoice.
    credit_of     uuid REFERENCES billing_invoices (id) ON DELETE SET NULL,
    status        text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'approved', 'sent', 'void')),
    on_hold       boolean NOT NULL DEFAULT false,
    hold_reason   text CHECK (length(hold_reason) <= 500),
    currency      text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    period_start  date,
    period_end    date,
    issued_on     date,
    due_on        date,
    notes         text CHECK (length(notes) <= 4000),
    run_id        uuid REFERENCES billing_runs (id) ON DELETE SET NULL,
    created_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    approved_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    approved_at   timestamptz,
    voided_at     timestamptz,
    version       integer NOT NULL DEFAULT 1,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX billing_invoices_org ON billing_invoices (org_id, created_at DESC);

CREATE TABLE billing_invoice_lines (
    id           uuid PRIMARY KEY,
    invoice_id   uuid NOT NULL REFERENCES billing_invoices (id) ON DELETE CASCADE,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    position     integer NOT NULL,
    kind         text NOT NULL CHECK (kind IN ('time', 'product', 'recurring', 'project', 'manual')),
    description  text NOT NULL CHECK (length(btrim(description)) BETWEEN 1 AND 1000),
    quantity     numeric(18,4) NOT NULL,
    unit         text NOT NULL DEFAULT 'each' CHECK (length(unit) <= 32),
    unit_price   numeric(18,4) NOT NULL DEFAULT 0,
    product_id   uuid REFERENCES catalog_products (id) ON DELETE SET NULL,
    tax_code     text CHECK (length(tax_code) <= 50),
    -- Changed by hand after it was generated (or added by hand).
    edited       boolean NOT NULL DEFAULT false
);
CREATE INDEX billing_invoice_lines_invoice ON billing_invoice_lines (invoice_id, position);

-- What's been billed: each source once (per period, for recurring charges).
CREATE TABLE billing_billed (
    source_type  text NOT NULL CHECK (source_type IN ('time_entry', 'sales_order_line', 'agreement_line',
                                                      'project_fee', 'project_milestone')),
    source_id    uuid NOT NULL,
    period       date NOT NULL DEFAULT '-infinity',
    line_id      uuid NOT NULL REFERENCES billing_invoice_lines (id) ON DELETE CASCADE,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- For time and not-to-exceed caps: the amount billed for it.
    amount       numeric(18,4) NOT NULL DEFAULT 0,
    PRIMARY KEY (source_type, source_id, period)
);
CREATE INDEX billing_billed_line ON billing_billed (line_id);

-- What projects bill: a fixed fee (or a not-to-exceed cap), and milestones' amounts.
ALTER TABLE prj_projects
    ADD COLUMN fee_amount numeric(18,4) CHECK (fee_amount >= 0),
    ADD COLUMN fee_currency text NOT NULL DEFAULT 'USD' CHECK (fee_currency ~ '^[A-Z]{3}$');
ALTER TABLE prj_milestones ADD COLUMN amount numeric(18,4) CHECK (amount >= 0);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['billing_settings', 'billing_runs', 'billing_invoices', 'billing_invoice_lines',
                             'billing_billed'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;

