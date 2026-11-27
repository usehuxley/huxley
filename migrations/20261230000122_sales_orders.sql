-- Sales orders (design doc §8.10, ADR-069): an accepted quote becomes a
-- sales order at the client, with the lines agreed to. It's fulfilled from
-- stock (allocated, then used), with purchase orders for what's short, and
-- handed off: recurring lines to an agreement, the work to a project.

CREATE SEQUENCE sales_order_number;

CREATE TABLE sales_orders (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    number          bigint NOT NULL UNIQUE DEFAULT nextval('sales_order_number'),
    quote_id        uuid UNIQUE REFERENCES sales_quotes (id) ON DELETE SET NULL,
    opportunity_id  uuid REFERENCES sales_opportunities (id) ON DELETE SET NULL,
    title           text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
    currency        text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    status          text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'fulfilled', 'closed', 'cancelled')),
    agreement_id    uuid REFERENCES contract_agreements (id) ON DELETE SET NULL,
    project_id      uuid REFERENCES prj_projects (id) ON DELETE SET NULL,
    notes           text CHECK (length(notes) <= 4000),
    created_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    closed_at       timestamptz,
    version         integer NOT NULL DEFAULT 1
);
CREATE INDEX sales_orders_org ON sales_orders (org_id, created_at DESC);

CREATE TABLE sales_order_lines (
    id            uuid PRIMARY KEY,
    order_id      uuid NOT NULL REFERENCES sales_orders (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    position      integer NOT NULL,
    product_id    uuid REFERENCES catalog_products (id) ON DELETE SET NULL,
    sku           text,
    name          text NOT NULL,
    description   text NOT NULL DEFAULT '',
    billing       text NOT NULL CHECK (billing IN ('one_time', 'monthly', 'quarterly', 'annual')),
    quantity      numeric(18,4) NOT NULL CHECK (quantity > 0),
    unit          text NOT NULL DEFAULT 'each',
    unit_price    numeric(18,4) NOT NULL DEFAULT 0,
    discount_pct  numeric(7,4) NOT NULL DEFAULT 0,
    unit_cost     numeric(18,4) NOT NULL DEFAULT 0,
    -- How much has been delivered (or, for recurring lines, handed to an agreement).
    fulfilled     numeric(18,4) NOT NULL DEFAULT 0 CHECK (fulfilled >= 0 AND fulfilled <= quantity)
);
CREATE INDEX sales_order_lines_order ON sales_order_lines (order_id, position);

ALTER TABLE sales_orders ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales_order_lines ENABLE ROW LEVEL SECURITY;
CREATE POLICY sales_orders_all ON sales_orders FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
CREATE POLICY sales_order_lines_all ON sales_order_lines FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Stock is allocated to, and bought for, sales orders too.
ALTER TABLE inventory_allocations DROP CONSTRAINT inventory_allocations_ref_type_check,
    ADD CONSTRAINT inventory_allocations_ref_type_check CHECK (ref_type IN ('ticket', 'project', 'sales_order'));
ALTER TABLE procurement_order_lines DROP CONSTRAINT procurement_order_lines_for_ref_type_check,
    ADD CONSTRAINT procurement_order_lines_for_ref_type_check
        CHECK (for_ref_type IN ('ticket', 'project', 'sales_order'));

-- Accepted quotes from before sales orders get theirs.
INSERT INTO sales_orders (id, org_id, quote_id, opportunity_id, title, currency, created_by, created_at)
SELECT gen_random_uuid(), q.org_id, q.id, q.opportunity_id, q.title, q.currency, q.created_by, coalesce(q.decided_at, now())
  FROM sales_quotes q WHERE q.status = 'accepted';
INSERT INTO sales_order_lines (id, order_id, org_id, position, product_id, sku, name, description, billing, quantity,
                               unit, unit_price, discount_pct, unit_cost)
SELECT gen_random_uuid(), o.id, o.org_id, l.position, l.product_id, l.sku, l.name, l.description, l.billing,
       l.quantity, l.unit, l.unit_price, l.discount_pct, l.unit_cost
  FROM sales_quote_lines l JOIN sales_orders o ON o.quote_id = l.quote_id
 WHERE l.kind = 'item' AND (NOT l.optional OR l.selected) AND l.quantity > 0;
