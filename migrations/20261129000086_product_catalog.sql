-- The product catalog (design doc §8.7, the contracts module): products and
-- services an MSP sells, with SKUs, vendors, cost and price, and bundles of
-- them. Money is numeric(18,4) with an explicit currency (§8.11).

CREATE TABLE catalog_products (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    sku               text NOT NULL CHECK (sku ~ '^[A-Za-z0-9][A-Za-z0-9._/-]{0,63}$'),
    name              text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    description       text NOT NULL DEFAULT '' CHECK (length(description) <= 4000),
    kind              text NOT NULL CHECK (kind IN ('product', 'service', 'bundle')),
    category          text CHECK (length(category) BETWEEN 1 AND 100),
    vendor            text CHECK (length(vendor) BETWEEN 1 AND 200),
    manufacturer_part text CHECK (length(manufacturer_part) BETWEEN 1 AND 100),
    -- What one is: each, hour, month, user, device…
    unit              text NOT NULL DEFAULT 'each' CHECK (length(unit) BETWEEN 1 AND 32),
    currency          text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    cost              numeric(18,4) CHECK (cost >= 0),
    -- None for a bundle: its components' prices add up.
    price             numeric(18,4) CHECK (price >= 0),
    billing           text NOT NULL DEFAULT 'one_time'
                      CHECK (billing IN ('one_time', 'monthly', 'quarterly', 'annual')),
    taxable           boolean NOT NULL DEFAULT true,
    active            boolean NOT NULL DEFAULT true,
    version           integer NOT NULL DEFAULT 1,
    created_by        uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX catalog_products_sku ON catalog_products (org_id, lower(sku));
CREATE INDEX catalog_products_name ON catalog_products USING gin (name gin_trgm_ops);

-- A bundle's components: other products (not bundles) of its organization.
CREATE TABLE catalog_bundle_items (
    bundle_id   uuid NOT NULL REFERENCES catalog_products (id) ON DELETE CASCADE,
    product_id  uuid NOT NULL REFERENCES catalog_products (id) ON DELETE RESTRICT,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    quantity    numeric(18,4) NOT NULL CHECK (quantity > 0),
    position    integer NOT NULL DEFAULT 0,
    PRIMARY KEY (bundle_id, product_id),
    CHECK (bundle_id <> product_id)
);
CREATE INDEX catalog_bundle_items_product ON catalog_bundle_items (product_id);

ALTER TABLE catalog_products ENABLE ROW LEVEL SECURITY;
CREATE POLICY catalog_products_all ON catalog_products FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
ALTER TABLE catalog_bundle_items ENABLE ROW LEVEL SECURITY;
CREATE POLICY catalog_bundle_items_all ON catalog_bundle_items FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Technicians and read-only users can look things up.
UPDATE roles SET permissions = permissions || '{catalog:read}'
 WHERE key IN ('technician', 'read_only') AND is_system;
