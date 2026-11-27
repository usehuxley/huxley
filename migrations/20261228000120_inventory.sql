-- Inventory (design doc §8.7, ADR-067): stock of catalog products at
-- locations (warehouses, bins, technicians' vehicles, client sites), by
-- lot or serial number where the product is tracked that way; a ledger of
-- every movement (receive, transfer, adjust, allocate, release, consume,
-- return, RMA); parts allocated to tickets and projects and then used or
-- released; and reorder points that raise `stock.low`.
--
-- Stock belongs to the organization that holds the location (usually the
-- provider); allocations name the client record they're for.

CREATE TABLE inventory_locations (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name              text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    kind              text NOT NULL CHECK (kind IN ('warehouse', 'bin', 'vehicle', 'site')),
    -- A bin sits in a warehouse.
    parent_id         uuid REFERENCES inventory_locations (id) ON DELETE RESTRICT,
    -- A vehicle's technician.
    assigned_user_id  uuid REFERENCES users (id) ON DELETE SET NULL,
    address           text CHECK (length(address) <= 500),
    active            boolean NOT NULL DEFAULT true,
    created_at        timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, name)
);

-- A catalog product an organization stocks, and how it's tracked. Its
-- SKU, name and unit are copied from the catalog (which may be an
-- ancestor's, out of the stocking organization's sight).
CREATE TABLE inventory_items (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    product_id   uuid NOT NULL REFERENCES catalog_products (id) ON DELETE CASCADE,
    sku          text NOT NULL,
    name         text NOT NULL,
    unit         text NOT NULL DEFAULT 'each',
    tracking     text NOT NULL DEFAULT 'none' CHECK (tracking IN ('none', 'serial', 'lot')),
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, product_id)
);

-- What's on hand at a location (per lot, '' when not lot-tracked), and how
-- much of it is allocated.
CREATE TABLE inventory_stock (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    item_id      uuid NOT NULL REFERENCES inventory_items (id) ON DELETE CASCADE,
    location_id  uuid NOT NULL REFERENCES inventory_locations (id) ON DELETE RESTRICT,
    lot          text NOT NULL DEFAULT '' CHECK (length(lot) <= 100),
    on_hand      numeric(18,4) NOT NULL DEFAULT 0 CHECK (on_hand >= 0),
    allocated    numeric(18,4) NOT NULL DEFAULT 0 CHECK (allocated >= 0 AND allocated <= on_hand),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (item_id, location_id, lot)
);
CREATE INDEX inventory_stock_location ON inventory_stock (location_id);

-- Reorder point and quantity for an item at a location; `low_since` is
-- set while what's available is at or under the point (one stock.low per dip).
CREATE TABLE inventory_levels (
    item_id        uuid NOT NULL REFERENCES inventory_items (id) ON DELETE CASCADE,
    location_id    uuid NOT NULL REFERENCES inventory_locations (id) ON DELETE CASCADE,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    reorder_point  numeric(18,4) NOT NULL CHECK (reorder_point >= 0),
    reorder_qty    numeric(18,4) NOT NULL DEFAULT 0 CHECK (reorder_qty >= 0),
    low_since      timestamptz,
    PRIMARY KEY (item_id, location_id)
);

-- Parts set aside for a ticket or project, then used (consumed) or released.
CREATE TABLE inventory_allocations (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    item_id       uuid NOT NULL REFERENCES inventory_items (id) ON DELETE CASCADE,
    location_id   uuid NOT NULL REFERENCES inventory_locations (id) ON DELETE RESTRICT,
    lot           text NOT NULL DEFAULT '',
    quantity      numeric(18,4) NOT NULL CHECK (quantity > 0),
    serials       text[] NOT NULL DEFAULT '{}',
    ref_type      text NOT NULL CHECK (ref_type IN ('ticket', 'project')),
    ref_id        uuid NOT NULL,
    -- The client organization of the ticket or project.
    ref_org_id    uuid REFERENCES organizations (id) ON DELETE SET NULL,
    status        text NOT NULL DEFAULT 'allocated' CHECK (status IN ('allocated', 'consumed', 'released')),
    note          text CHECK (length(note) <= 1000),
    created_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    closed_at     timestamptz
);
CREATE INDEX inventory_allocations_ref ON inventory_allocations (ref_type, ref_id);

-- Serial-numbered units, wherever they are.
CREATE TABLE inventory_units (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    item_id        uuid NOT NULL REFERENCES inventory_items (id) ON DELETE CASCADE,
    serial         text NOT NULL CHECK (length(btrim(serial)) BETWEEN 1 AND 100),
    -- None once it's left stock (consumed or sent back).
    location_id    uuid REFERENCES inventory_locations (id) ON DELETE RESTRICT,
    status         text NOT NULL CHECK (status IN ('in_stock', 'allocated', 'consumed', 'rma', 'returned')),
    allocation_id  uuid REFERENCES inventory_allocations (id) ON DELETE SET NULL,
    updated_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (item_id, serial)
);

-- Every movement, in order. Never changed.
CREATE TABLE inventory_movements (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    item_id        uuid NOT NULL REFERENCES inventory_items (id) ON DELETE CASCADE,
    kind           text NOT NULL CHECK (kind IN ('receive', 'transfer', 'adjust', 'allocate', 'release',
                                                 'consume', 'return', 'rma')),
    -- Signed for adjustments; positive otherwise.
    quantity       numeric(18,4) NOT NULL,
    from_location  uuid REFERENCES inventory_locations (id) ON DELETE SET NULL,
    to_location    uuid REFERENCES inventory_locations (id) ON DELETE SET NULL,
    lot            text NOT NULL DEFAULT '',
    serials        text[] NOT NULL DEFAULT '{}',
    unit_cost      numeric(18,4),
    allocation_id  uuid REFERENCES inventory_allocations (id) ON DELETE SET NULL,
    ref_type       text,
    ref_id         uuid,
    reference      text CHECK (length(reference) <= 200),
    note           text CHECK (length(note) <= 1000),
    created_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX inventory_movements_item ON inventory_movements (item_id, created_at DESC);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['inventory_locations', 'inventory_items', 'inventory_stock', 'inventory_levels',
                             'inventory_allocations', 'inventory_units', 'inventory_movements'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;

-- Technicians take parts for their work; read-only users and sales look.
UPDATE roles SET permissions = permissions || '{inventory:read,inventory:write}'
 WHERE key = 'technician' AND is_system;
UPDATE roles SET permissions = permissions || '{inventory:read}'
 WHERE key IN ('read_only', 'sales') AND is_system;
