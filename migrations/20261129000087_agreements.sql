-- Agreements (design doc §8.7, ADR-039): what a client has agreed to, what
-- it covers, the rates for billable time, and recurring billing lines.
-- Time entries are evaluated against them on demand.

CREATE TABLE contract_agreements (
    id                       uuid PRIMARY KEY,
    org_id                   uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name                     text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    kind                     text NOT NULL
                             CHECK (kind IN ('managed', 'block_hours', 'time_materials', 'project')),
    status                   text NOT NULL DEFAULT 'draft'
                             CHECK (status IN ('draft', 'active', 'cancelled')),
    starts_on                date NOT NULL,
    ends_on                  date CHECK (ends_on IS NULL OR ends_on >= starts_on),
    auto_renew               boolean NOT NULL DEFAULT false,
    renewal_notice_days      integer NOT NULL DEFAULT 30 CHECK (renewal_notice_days BETWEEN 0 AND 365),
    currency                 text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    -- Lower goes first when several apply at the same organization.
    priority                 integer NOT NULL DEFAULT 0 CHECK (priority BETWEEN 0 AND 1000),
    -- Coverage (managed and block_hours).
    covers_all_work_types    boolean NOT NULL DEFAULT true,
    -- managed: included minutes a calendar month (none: unlimited).
    monthly_minutes          integer CHECK (monthly_minutes > 0),
    -- block_hours: minutes bought for the whole term.
    block_minutes            integer CHECK (block_minutes > 0),
    billing_increment        integer NOT NULL DEFAULT 1 CHECK (billing_increment BETWEEN 1 AND 60),
    notes                    text NOT NULL DEFAULT '' CHECK (length(notes) <= 4000),
    version                  integer NOT NULL DEFAULT 1,
    created_by               uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at               timestamptz NOT NULL DEFAULT now(),
    updated_at               timestamptz NOT NULL DEFAULT now(),
    CHECK (kind = 'block_hours' OR block_minutes IS NULL),
    CHECK (kind = 'block_hours' OR kind = 'managed' OR monthly_minutes IS NULL),
    CHECK (kind <> 'block_hours' OR block_minutes IS NOT NULL),
    CHECK (kind <> 'managed' OR block_minutes IS NULL)
);
CREATE INDEX contract_agreements_org ON contract_agreements (org_id, starts_on);

-- The work types it covers, when not all.
CREATE TABLE contract_agreement_work_types (
    agreement_id  uuid NOT NULL REFERENCES contract_agreements (id) ON DELETE CASCADE,
    work_type_id  uuid NOT NULL REFERENCES sd_work_types (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    PRIMARY KEY (agreement_id, work_type_id)
);

-- The asset types it covers (none: any).
CREATE TABLE contract_agreement_asset_types (
    agreement_id   uuid NOT NULL REFERENCES contract_agreements (id) ON DELETE CASCADE,
    asset_type_id  uuid NOT NULL REFERENCES cmdb_asset_types (id) ON DELETE CASCADE,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    PRIMARY KEY (agreement_id, asset_type_id)
);

-- The rate card: an hourly rate per work type, and a default (no work type).
CREATE TABLE contract_rates (
    agreement_id  uuid NOT NULL REFERENCES contract_agreements (id) ON DELETE CASCADE,
    work_type_id  uuid REFERENCES sd_work_types (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    hourly_rate   numeric(18,4) NOT NULL CHECK (hourly_rate >= 0)
);
CREATE UNIQUE INDEX contract_rates_one
    ON contract_rates (agreement_id, COALESCE(work_type_id, '00000000-0000-0000-0000-000000000000'));

-- Recurring billing lines: the agreement's fee.
CREATE TABLE contract_lines (
    id            uuid PRIMARY KEY,
    agreement_id  uuid NOT NULL REFERENCES contract_agreements (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    product_id    uuid REFERENCES catalog_products (id) ON DELETE RESTRICT,
    description   text NOT NULL CHECK (length(btrim(description)) BETWEEN 1 AND 500),
    quantity      numeric(18,4) NOT NULL CHECK (quantity > 0),
    -- None: the product's price.
    unit_price    numeric(18,4) CHECK (unit_price >= 0),
    billing       text NOT NULL DEFAULT 'monthly'
                  CHECK (billing IN ('monthly', 'quarterly', 'annual')),
    position      integer NOT NULL DEFAULT 0
);
CREATE INDEX contract_lines_agreement ON contract_lines (agreement_id, position);
CREATE INDEX contract_lines_product ON contract_lines (product_id);

DO $$
DECLARE tbl text;
BEGIN
    FOREACH tbl IN ARRAY ARRAY['contract_agreements', 'contract_agreement_work_types',
                               'contract_agreement_asset_types', 'contract_rates', 'contract_lines']
    LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', tbl);
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app '
            'USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[])) '
            'WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            tbl || '_all', tbl);
    END LOOP;
END $$;

-- Technicians see what a client's agreement covers.
UPDATE roles SET permissions = permissions || '{agreement:read}'
 WHERE key IN ('technician', 'read_only') AND is_system;
