-- Sales pipeline (design doc §8.10, ADR-063): leads, opportunities moving
-- through configurable stages, the stage history, and win/loss reasons.
--
-- - A lead is someone who may buy, held at the provider organization that
--   captured it. Converting one creates the company (a client organization,
--   relationship "prospect" unless it already exists), the contact and an
--   opportunity.
-- - An opportunity belongs to the client organization it would sell to.
-- - Stages and reasons are instance-wide, like ticket types.

CREATE TABLE sales_stages (
    id           uuid PRIMARY KEY,
    name         text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 60),
    -- open stages are worked; won and lost close the opportunity.
    kind         text NOT NULL CHECK (kind IN ('open', 'won', 'lost')),
    -- The chance of winning at this stage, for the weighted pipeline.
    probability  smallint NOT NULL CHECK (probability BETWEEN 0 AND 100),
    position     integer NOT NULL DEFAULT 0,
    archived_at  timestamptz,
    CHECK (kind <> 'won' OR probability = 100),
    CHECK (kind <> 'lost' OR probability = 0)
);
CREATE UNIQUE INDEX sales_stages_name ON sales_stages (lower(name)) WHERE archived_at IS NULL;

INSERT INTO sales_stages (id, name, kind, probability, position) VALUES
    (gen_random_uuid(), 'Qualify', 'open', 10, 1),
    (gen_random_uuid(), 'Discover', 'open', 25, 2),
    (gen_random_uuid(), 'Propose', 'open', 50, 3),
    (gen_random_uuid(), 'Negotiate', 'open', 75, 4),
    (gen_random_uuid(), 'Won', 'won', 100, 5),
    (gen_random_uuid(), 'Lost', 'lost', 0, 6);

CREATE TABLE sales_reasons (
    id           uuid PRIMARY KEY,
    kind         text NOT NULL CHECK (kind IN ('won', 'lost')),
    name         text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    position     integer NOT NULL DEFAULT 0,
    archived_at  timestamptz
);
CREATE UNIQUE INDEX sales_reasons_name ON sales_reasons (kind, lower(name)) WHERE archived_at IS NULL;

INSERT INTO sales_reasons (id, kind, name, position) VALUES
    (gen_random_uuid(), 'won', 'Price', 1),
    (gen_random_uuid(), 'won', 'Relationship', 2),
    (gen_random_uuid(), 'won', 'Service and support', 3),
    (gen_random_uuid(), 'lost', 'Price', 1),
    (gen_random_uuid(), 'lost', 'Chose a competitor', 2),
    (gen_random_uuid(), 'lost', 'No decision', 3),
    (gen_random_uuid(), 'lost', 'No budget', 4),
    (gen_random_uuid(), 'lost', 'Timing', 5);

CREATE TABLE sales_leads (
    id                     uuid PRIMARY KEY,
    -- The provider organization that captured it.
    org_id                 uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    company_name           text NOT NULL CHECK (length(btrim(company_name)) BETWEEN 1 AND 200),
    first_name             text CHECK (length(first_name) <= 100),
    last_name              text CHECK (length(last_name) <= 100),
    email                  text CHECK (length(email) <= 254),
    phone                  text CHECK (length(phone) <= 50),
    -- Where it came from: web form, referral, event, email, api…
    source                 text CHECK (length(source) BETWEEN 1 AND 60),
    notes                  text NOT NULL DEFAULT '' CHECK (length(notes) <= 8000),
    status                 text NOT NULL DEFAULT 'new'
                           CHECK (status IN ('new', 'working', 'disqualified', 'converted')),
    disqualified_reason    text CHECK (length(disqualified_reason) <= 500),
    owner_id               uuid REFERENCES users (id) ON DELETE SET NULL,
    -- Set when converted.
    converted_org_id       uuid REFERENCES organizations (id) ON DELETE SET NULL,
    converted_contact_id   uuid,
    converted_opportunity_id uuid,
    converted_at           timestamptz,
    version                integer NOT NULL DEFAULT 1,
    created_by             uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at             timestamptz NOT NULL DEFAULT now(),
    updated_at             timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sales_leads_org ON sales_leads (org_id, status);
CREATE INDEX sales_leads_company ON sales_leads USING gin (company_name gin_trgm_ops);

CREATE TABLE sales_opportunities (
    id              uuid PRIMARY KEY,
    -- The client organization it would sell to.
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    number          bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    name            text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    description     text NOT NULL DEFAULT '' CHECK (length(description) <= 8000),
    stage_id        uuid NOT NULL REFERENCES sales_stages (id),
    status          text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'won', 'lost')),
    -- Expected one-time and monthly recurring value.
    amount          numeric(18,4) NOT NULL DEFAULT 0 CHECK (amount >= 0),
    recurring       numeric(18,4) NOT NULL DEFAULT 0 CHECK (recurring >= 0),
    currency        text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    -- Overrides the stage's probability when set.
    probability     smallint CHECK (probability BETWEEN 0 AND 100),
    close_date      date,
    -- The forecast category: pipeline, best case or commit.
    forecast        text NOT NULL DEFAULT 'pipeline'
                    CHECK (forecast IN ('pipeline', 'best_case', 'commit', 'omitted')),
    owner_id        uuid REFERENCES users (id) ON DELETE SET NULL,
    contact_id      uuid REFERENCES crm_contacts (id) ON DELETE SET NULL,
    lead_id         uuid REFERENCES sales_leads (id) ON DELETE SET NULL,
    source          text CHECK (length(source) BETWEEN 1 AND 60),
    reason_id       uuid REFERENCES sales_reasons (id),
    reason_note     text CHECK (length(reason_note) <= 2000),
    closed_at       timestamptz,
    version         integer NOT NULL DEFAULT 1,
    created_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    CHECK ((status = 'open') = (closed_at IS NULL))
);
CREATE INDEX sales_opportunities_org ON sales_opportunities (org_id);
CREATE INDEX sales_opportunities_stage ON sales_opportunities (stage_id) WHERE status = 'open';
CREATE INDEX sales_opportunities_owner ON sales_opportunities (owner_id, status);
CREATE INDEX sales_opportunities_name ON sales_opportunities USING gin (name gin_trgm_ops);

ALTER TABLE sales_leads
    ADD CONSTRAINT sales_leads_opportunity FOREIGN KEY (converted_opportunity_id)
        REFERENCES sales_opportunities (id) ON DELETE SET NULL;

-- Each move between stages, for history and time-in-stage.
CREATE TABLE sales_stage_history (
    id              uuid PRIMARY KEY,
    opportunity_id  uuid NOT NULL REFERENCES sales_opportunities (id) ON DELETE CASCADE,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    from_stage_id   uuid REFERENCES sales_stages (id),
    to_stage_id     uuid NOT NULL REFERENCES sales_stages (id),
    actor_id        uuid REFERENCES users (id) ON DELETE SET NULL,
    at              timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sales_stage_history_opportunity ON sales_stage_history (opportunity_id, at);

ALTER TABLE sales_leads ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales_opportunities ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales_stage_history ENABLE ROW LEVEL SECURITY;
DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['sales_leads', 'sales_opportunities', 'sales_stage_history'] LOOP
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;

-- A Sales role for account managers and sales engineers; read-only users
-- can look.
INSERT INTO roles (id, key, name, description, permissions, is_system)
VALUES (gen_random_uuid(), 'sales', 'Sales',
        'Works leads and opportunities, and looks up clients and the catalog.',
        '{sales:read,sales:write,crm:read,crm:manage,catalog:read,org:read}', true)
ON CONFLICT (key) DO NOTHING;
UPDATE roles SET permissions = permissions || '{sales:read}'
 WHERE key = 'read_only' AND is_system;
