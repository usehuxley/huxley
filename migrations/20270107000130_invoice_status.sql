-- Payment status back-sync (design doc §8.11, ADR-078): the sync pack reads
-- what's paid and due in the accounting system onto each synced invoice;
-- Huxley keeps each client's balance and overdue amount, raises
-- invoice.paid, and (optionally) puts clients on credit hold.

ALTER TABLE billing_invoice_syncs
    ADD COLUMN amount_due      numeric(18,4),
    ADD COLUMN amount_paid     numeric(18,4),
    -- The accounting system's own word for it (e.g. PAID, AUTHORISED).
    ADD COLUMN remote_status   text CHECK (length(remote_status) <= 50),
    -- Where the client views and pays it.
    ADD COLUMN pay_url         text CHECK (length(pay_url) <= 2000),
    ADD COLUMN paid_on         date,
    ADD COLUMN status_checked_at timestamptz;

-- When a client goes on credit hold: an invoice overdue this many days, or
-- this much overdue in all (either; none of both: never).
ALTER TABLE billing_sync_settings
    ADD COLUMN credit_hold_days    integer CHECK (credit_hold_days BETWEEN 1 AND 365),
    ADD COLUMN credit_hold_amount  numeric(18,4) CHECK (credit_hold_amount >= 0);

-- Each client's position, from its synced invoices.
CREATE TABLE billing_client_status (
    org_id        uuid PRIMARY KEY REFERENCES organizations (id) ON DELETE CASCADE,
    currency      text NOT NULL DEFAULT 'USD',
    balance       numeric(18,4) NOT NULL DEFAULT 0,
    overdue       numeric(18,4) NOT NULL DEFAULT 0,
    oldest_due    date,
    credit_hold   boolean NOT NULL DEFAULT false,
    hold_since    timestamptz,
    updated_at    timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE billing_client_status ENABLE ROW LEVEL SECURITY;
CREATE POLICY billing_client_status_all ON billing_client_status FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
