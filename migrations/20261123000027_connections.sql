-- Connections (design doc §7.2): an integration's auth scheme with its
-- credentials, owned by an org and optionally shared with the orgs below.
-- Secret fields (passwords, keys, client secrets, tokens) are encrypted
-- with the org's data key; everything else is plain settings.

CREATE TYPE connection_status AS ENUM ('ready', 'needs_auth', 'error');

CREATE TABLE connections (
    id                  uuid PRIMARY KEY,
    org_id              uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    integration_id      uuid NOT NULL REFERENCES integrations (id) ON DELETE CASCADE,
    key                 text NOT NULL CHECK (key ~ '^[a-z][a-z0-9-]{0,63}$'),
    name                text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    -- The integration's auth scheme key; NULL for APIs that need none.
    auth_scheme         text,
    -- Overrides the integration's server URL (another region, a sandbox).
    base_url            text,
    server_variables    jsonb NOT NULL DEFAULT '{}',
    settings            jsonb NOT NULL DEFAULT '{}',
    secrets_sealed      bytea,
    -- Which secret fields are set, so they can be shown without decrypting.
    secret_fields       text[] NOT NULL DEFAULT '{}',
    shared              boolean NOT NULL DEFAULT false,
    status              connection_status NOT NULL DEFAULT 'ready',
    status_message      text,
    created_by          uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_by          uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, key)
);

CREATE INDEX connections_integration ON connections (integration_id);

ALTER TABLE connections ENABLE ROW LEVEL SECURITY;
CREATE POLICY connections_all ON connections FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));

-- Technicians can see which connections exist (never their secrets).
UPDATE roles SET permissions = permissions || '{connection:read}'
 WHERE key = 'technician' AND is_system;
