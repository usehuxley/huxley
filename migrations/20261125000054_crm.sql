-- The `crm` module's base (design doc §8.1, ADR-035): company details,
-- sites and contacts. Everything belongs to an organization, and row-level
-- security follows it, in the InitPlan form (migration 050).

-- One row per organization that has company details.
CREATE TABLE crm_companies (
    org_id        uuid PRIMARY KEY REFERENCES organizations (id) ON DELETE CASCADE,
    relationship  text NOT NULL DEFAULT 'client'
                  CHECK (relationship IN ('client', 'vendor', 'partner', 'prospect')),
    status        text NOT NULL DEFAULT 'active'
                  CHECK (status IN ('active', 'inactive', 'onboarding', 'offboarding')),
    website       text CHECK (length(website) <= 500),
    phone         text CHECK (length(phone) <= 50),
    notes         text CHECK (length(notes) <= 10000),
    version       integer NOT NULL DEFAULT 1,
    updated_at    timestamptz NOT NULL DEFAULT now(),
    updated_by    uuid REFERENCES users (id) ON DELETE SET NULL
);

CREATE TABLE crm_sites (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name          text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    address_line1 text CHECK (length(address_line1) <= 200),
    address_line2 text CHECK (length(address_line2) <= 200),
    city          text CHECK (length(city) <= 100),
    region        text CHECK (length(region) <= 100),
    postal_code   text CHECK (length(postal_code) <= 20),
    -- ISO 3166-1 alpha-2.
    country       text CHECK (country ~ '^[A-Z]{2}$'),
    -- IANA name, checked by the application.
    timezone      text NOT NULL DEFAULT 'UTC',
    -- {"mon": [["09:00","17:00"]], …}; a missing day is closed.
    hours         jsonb NOT NULL DEFAULT '{}'::jsonb,
    phone         text CHECK (length(phone) <= 50),
    is_primary    boolean NOT NULL DEFAULT false,
    custom_fields jsonb NOT NULL DEFAULT '{}'::jsonb,
    version       integer NOT NULL DEFAULT 1,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    archived_at   timestamptz
);
CREATE INDEX crm_sites_org ON crm_sites (org_id) WHERE archived_at IS NULL;
-- One primary site per organization.
CREATE UNIQUE INDEX crm_sites_primary ON crm_sites (org_id)
    WHERE is_primary AND archived_at IS NULL;

CREATE TABLE crm_contacts (
    id            uuid PRIMARY KEY,
    -- The primary company: row-level security and permissions follow it.
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    first_name    text NOT NULL DEFAULT '' CHECK (length(first_name) <= 100),
    last_name     text NOT NULL DEFAULT '' CHECK (length(last_name) <= 100),
    email         text CHECK (length(email) <= 320),
    phone         text CHECK (length(phone) <= 50),
    mobile        text CHECK (length(mobile) <= 50),
    -- Identities elsewhere, e.g. {"entra": "<object id>"}.
    external_ids  jsonb NOT NULL DEFAULT '{}'::jsonb,
    -- The person's portal account, once they have one.
    portal_user_id uuid REFERENCES users (id) ON DELETE SET NULL,
    site_id       uuid REFERENCES crm_sites (id) ON DELETE SET NULL,
    custom_fields jsonb NOT NULL DEFAULT '{}'::jsonb,
    version       integer NOT NULL DEFAULT 1,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    archived_at   timestamptz,
    CHECK (length(btrim(first_name || last_name)) > 0)
);
CREATE INDEX crm_contacts_org ON crm_contacts (org_id) WHERE archived_at IS NULL;
CREATE INDEX crm_contacts_email ON crm_contacts (lower(email)) WHERE archived_at IS NULL;

-- A contact's companies: the primary one, and any others.
CREATE TABLE crm_contact_companies (
    contact_id  uuid NOT NULL REFERENCES crm_contacts (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    job_title   text CHECK (length(job_title) <= 200),
    is_primary  boolean NOT NULL DEFAULT false,
    PRIMARY KEY (contact_id, org_id)
);
CREATE INDEX crm_contact_companies_org ON crm_contact_companies (org_id);
CREATE UNIQUE INDEX crm_contact_companies_primary ON crm_contact_companies (contact_id)
    WHERE is_primary;

ALTER TABLE crm_companies ENABLE ROW LEVEL SECURITY;
CREATE POLICY crm_companies_all ON crm_companies FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
ALTER TABLE crm_sites ENABLE ROW LEVEL SECURITY;
CREATE POLICY crm_sites_all ON crm_sites FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
ALTER TABLE crm_contacts ENABLE ROW LEVEL SECURITY;
CREATE POLICY crm_contacts_all ON crm_contacts FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
ALTER TABLE crm_contact_companies ENABLE ROW LEVEL SECURITY;
CREATE POLICY crm_contact_companies_all ON crm_contact_companies FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Technicians work with contacts and sites; read-only users can see them.
-- (Owners and admins hold `*`.)
UPDATE roles SET permissions = permissions || '{crm:read,crm:manage}'
 WHERE key = 'technician' AND is_system;
UPDATE roles SET permissions = permissions || '{crm:read}'
 WHERE key = 'read_only' AND is_system;
