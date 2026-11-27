-- Purchasing (design doc §8.10, ADR-068): vendors; purchase orders with
-- lines, approved above a threshold, sent to the vendor and received into
-- inventory (in part or in full); lines bought for a ticket or project are
-- allocated to it on arrival. Low stock suggests orders per vendor.

CREATE TABLE procurement_vendors (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name            text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    account_number  text CHECK (length(account_number) <= 100),
    email           text CHECK (length(email) <= 320),
    phone           text CHECK (length(phone) <= 50),
    website         text CHECK (length(website) <= 500),
    address         text CHECK (length(address) <= 1000),
    currency        text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    payment_terms   text CHECK (length(payment_terms) <= 200),
    notes           text CHECK (length(notes) <= 4000),
    active          boolean NOT NULL DEFAULT true,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX procurement_vendors_name ON procurement_vendors (org_id, lower(name));

CREATE SEQUENCE procurement_order_number;

CREATE TABLE procurement_orders (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    number            bigint NOT NULL UNIQUE DEFAULT nextval('procurement_order_number'),
    vendor_id         uuid NOT NULL REFERENCES procurement_vendors (id) ON DELETE RESTRICT,
    status            text NOT NULL DEFAULT 'draft'
                      CHECK (status IN ('draft', 'submitted', 'approved', 'ordered', 'partially_received',
                                        'received', 'cancelled')),
    currency          text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    expected_on       date,
    -- Where deliveries go, unless received elsewhere.
    ship_to           uuid REFERENCES inventory_locations (id) ON DELETE SET NULL,
    vendor_reference  text CHECK (length(vendor_reference) <= 200),
    notes             text CHECK (length(notes) <= 4000),
    -- Why it was sent back to draft.
    rejection         text CHECK (length(rejection) <= 2000),
    created_by        uuid REFERENCES users (id) ON DELETE SET NULL,
    submitted_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    submitted_at      timestamptz,
    approved_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    approved_at       timestamptz,
    ordered_at        timestamptz,
    closed_at         timestamptz,
    version           integer NOT NULL DEFAULT 1,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX procurement_orders_vendor ON procurement_orders (vendor_id);

CREATE TABLE procurement_order_lines (
    id            uuid PRIMARY KEY,
    order_id      uuid NOT NULL REFERENCES procurement_orders (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    position      integer NOT NULL,
    -- A stocked item (received into inventory), or a line of its own.
    item_id       uuid REFERENCES inventory_items (id) ON DELETE SET NULL,
    sku           text CHECK (length(sku) <= 64),
    name          text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 300),
    quantity      numeric(18,4) NOT NULL CHECK (quantity > 0),
    unit_cost     numeric(18,4) NOT NULL DEFAULT 0 CHECK (unit_cost >= 0),
    received      numeric(18,4) NOT NULL DEFAULT 0 CHECK (received >= 0 AND received <= quantity),
    -- Bought for a ticket or project: allocated to it when it arrives.
    for_ref_type  text CHECK (for_ref_type IN ('ticket', 'project')),
    for_ref_id    uuid,
    CHECK ((for_ref_type IS NULL) = (for_ref_id IS NULL))
);
CREATE INDEX procurement_order_lines_order ON procurement_order_lines (order_id, position);
CREATE INDEX procurement_order_lines_item ON procurement_order_lines (item_id);

-- Each delivery taken in.
CREATE TABLE procurement_receipts (
    id           uuid PRIMARY KEY,
    order_id     uuid NOT NULL REFERENCES procurement_orders (id) ON DELETE CASCADE,
    line_id      uuid NOT NULL REFERENCES procurement_order_lines (id) ON DELETE CASCADE,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    quantity     numeric(18,4) NOT NULL CHECK (quantity > 0),
    location_id  uuid REFERENCES inventory_locations (id) ON DELETE SET NULL,
    lot          text NOT NULL DEFAULT '',
    serials      text[] NOT NULL DEFAULT '{}',
    received_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    received_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX procurement_receipts_order ON procurement_receipts (order_id, received_at);

-- Per organization: orders over this total need approval (none: never).
CREATE TABLE procurement_settings (
    org_id          uuid PRIMARY KEY REFERENCES organizations (id) ON DELETE CASCADE,
    approval_above  numeric(18,4) CHECK (approval_above >= 0)
);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['procurement_vendors', 'procurement_orders', 'procurement_order_lines',
                             'procurement_receipts', 'procurement_settings'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;

-- Technicians and read-only users see what's on order; admins hold everything.
UPDATE roles SET permissions = permissions || '{procurement:read}'
 WHERE key IN ('technician', 'read_only') AND is_system;
