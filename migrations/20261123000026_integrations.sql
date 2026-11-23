-- Integrations imported from OpenAPI specs (design doc §7.1, ADR-023).
-- Each import is an immutable version: the spec as given and the
-- definition Huxley derived from it, with the importer's warnings.

CREATE TABLE integrations (
    id               uuid PRIMARY KEY,
    org_id           uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    key              text NOT NULL CHECK (key ~ '^[a-z][a-z0-9-]{0,63}$'),
    name             text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    description      text,
    current_version  integer NOT NULL DEFAULT 1,
    created_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, key)
);

CREATE TABLE integration_versions (
    integration_id  uuid NOT NULL REFERENCES integrations (id) ON DELETE CASCADE,
    version         integer NOT NULL CHECK (version > 0),
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE RESTRICT,
    source_format   text NOT NULL,
    spec            jsonb NOT NULL,
    definition      jsonb NOT NULL,
    warnings        jsonb NOT NULL DEFAULT '[]',
    imported_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    imported_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (integration_id, version)
);

ALTER TABLE integrations ENABLE ROW LEVEL SECURITY;
CREATE POLICY integrations_all ON integrations FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
ALTER TABLE integration_versions ENABLE ROW LEVEL SECURITY;
CREATE POLICY integration_versions_all ON integration_versions FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));

-- Technicians and read-only users can see integrations; owners and admins
-- hold `*`.
UPDATE roles SET permissions = permissions || '{integration:read}'
 WHERE key IN ('technician', 'read_only') AND is_system;
