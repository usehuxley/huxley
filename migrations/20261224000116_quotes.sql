-- Quotes (design doc §8.10, ADR-064): what's offered to a client, line by
-- line from the catalog, with one-time and recurring lines, options the
-- client may take, discounts and margin. Sending one keeps a revision (a
-- snapshot of what was sent); changing a sent quote starts the next
-- revision. Accepting one wins its opportunity.

CREATE TABLE sales_quotes (
    id              uuid PRIMARY KEY,
    -- The client it's for.
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    number          bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    opportunity_id  uuid REFERENCES sales_opportunities (id) ON DELETE SET NULL,
    title           text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
    status          text NOT NULL DEFAULT 'draft'
                    CHECK (status IN ('draft', 'sent', 'accepted', 'declined')),
    -- 1 until first sent; each change after sending starts the next.
    revision        integer NOT NULL DEFAULT 1 CHECK (revision >= 1),
    currency        text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    valid_until     date,
    contact_id      uuid REFERENCES crm_contacts (id) ON DELETE SET NULL,
    owner_id        uuid REFERENCES users (id) ON DELETE SET NULL,
    -- Shown above and below the lines.
    intro           text NOT NULL DEFAULT '' CHECK (length(intro) <= 20000),
    terms           text NOT NULL DEFAULT '' CHECK (length(terms) <= 20000),
    sent_at         timestamptz,
    decided_at      timestamptz,
    -- Who accepted or declined, as they gave it, and why for a decline.
    decided_by      text CHECK (length(decided_by) <= 200),
    decline_reason  text CHECK (length(decline_reason) <= 2000),
    version         integer NOT NULL DEFAULT 1,
    created_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sales_quotes_org ON sales_quotes (org_id, status);
CREATE INDEX sales_quotes_opportunity ON sales_quotes (opportunity_id);

CREATE TABLE sales_quote_lines (
    id            uuid PRIMARY KEY,
    quote_id      uuid NOT NULL REFERENCES sales_quotes (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    position      integer NOT NULL,
    -- A heading groups the lines after it; an item is priced.
    kind          text NOT NULL DEFAULT 'item' CHECK (kind IN ('item', 'heading')),
    product_id    uuid REFERENCES catalog_products (id) ON DELETE SET NULL,
    sku           text CHECK (length(sku) <= 64),
    name          text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 300),
    description   text NOT NULL DEFAULT '' CHECK (length(description) <= 4000),
    billing       text NOT NULL DEFAULT 'one_time'
                  CHECK (billing IN ('one_time', 'monthly', 'quarterly', 'annual')),
    quantity      numeric(18,4) NOT NULL DEFAULT 1 CHECK (quantity >= 0),
    unit          text NOT NULL DEFAULT 'each' CHECK (length(unit) BETWEEN 1 AND 32),
    unit_cost     numeric(18,4) NOT NULL DEFAULT 0 CHECK (unit_cost >= 0),
    unit_price    numeric(18,4) NOT NULL DEFAULT 0 CHECK (unit_price >= 0),
    discount_pct  numeric(5,2) NOT NULL DEFAULT 0 CHECK (discount_pct BETWEEN 0 AND 100),
    taxable       boolean NOT NULL DEFAULT true,
    -- An option the client may take or leave; counted only when selected.
    optional      boolean NOT NULL DEFAULT false,
    selected      boolean NOT NULL DEFAULT true
);
CREATE INDEX sales_quote_lines_quote ON sales_quote_lines (quote_id, position);

-- What was sent, each time: the quote and its lines as they were.
CREATE TABLE sales_quote_revisions (
    id          uuid PRIMARY KEY,
    quote_id    uuid NOT NULL REFERENCES sales_quotes (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    revision    integer NOT NULL,
    snapshot    jsonb NOT NULL,
    sent_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    sent_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (quote_id, revision)
);

ALTER TABLE sales_quotes ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales_quote_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales_quote_revisions ENABLE ROW LEVEL SECURITY;
DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['sales_quotes', 'sales_quote_lines', 'sales_quote_revisions'] LOOP
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;
