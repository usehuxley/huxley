-- Accounting sync (design doc §8.11, ADR-077): approved invoices and credit
-- memos go to the accounting system through a pack's system workflow
-- (QuickBooks Online and Xero first). Huxley maps clients, items, tax
-- codes and classes to the accounting system's, prepares each invoice with
-- them, and keeps each invoice's sync status (an error queue, with retry).

-- Per provider: which accounting system (a pack's target key, e.g.
-- quickbooks or xero), and whether invoices sync as soon as they're approved.
CREATE TABLE billing_sync_settings (
    org_id        uuid PRIMARY KEY REFERENCES organizations (id) ON DELETE CASCADE,
    target        text NOT NULL CHECK (target ~ '^[a-z][a-z0-9-]{1,39}$'),
    enabled       boolean NOT NULL DEFAULT true,
    -- Sync each invoice when it's approved (otherwise, by hand).
    on_approval   boolean NOT NULL DEFAULT true,
    updated_at    timestamptz NOT NULL DEFAULT now()
);

-- Huxley's side ↔ the accounting system's. local_key: a client's id
-- (customer, class), product:<id> or kind:<line kind> (item), a tax code
-- (tax_code); 'default' for the fallback item, tax code or class.
CREATE TABLE billing_mappings (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    target       text NOT NULL,
    kind         text NOT NULL CHECK (kind IN ('customer', 'item', 'tax_code', 'class')),
    local_key    text NOT NULL CHECK (length(local_key) BETWEEN 1 AND 100),
    remote_id    text NOT NULL CHECK (length(btrim(remote_id)) BETWEEN 1 AND 200),
    remote_name  text CHECK (length(remote_name) <= 300),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, target, kind, local_key)
);

-- Each invoice's sync: waiting for the workflow, synced, or failed (the
-- error queue). Retrying asks again.
CREATE TABLE billing_invoice_syncs (
    invoice_id     uuid PRIMARY KEY REFERENCES billing_invoices (id) ON DELETE CASCADE,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    target         text NOT NULL,
    status         text NOT NULL CHECK (status IN ('pending', 'synced', 'failed')),
    remote_id      text CHECK (length(remote_id) <= 200),
    remote_number  text CHECK (length(remote_number) <= 100),
    -- The accounting system's page for it.
    remote_url     text CHECK (length(remote_url) <= 2000),
    attempts       integer NOT NULL DEFAULT 0,
    error          text CHECK (length(error) <= 4000),
    requested_at   timestamptz NOT NULL DEFAULT now(),
    synced_at      timestamptz
);
CREATE INDEX billing_invoice_syncs_status ON billing_invoice_syncs (status, requested_at);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['billing_sync_settings', 'billing_mappings', 'billing_invoice_syncs'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;
