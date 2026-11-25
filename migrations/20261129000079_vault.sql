-- The credential vault (design doc §8.5, §11 "Secrets at rest"): an
-- optional module keeping client passwords, keys and secret notes. Each
-- secret is sealed with the organization's data key (envelope encryption,
-- like connection credentials), never listed, and every reveal is logged
-- and audited.

CREATE TABLE vault_items (
    id                uuid PRIMARY KEY,
    org_id            uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name              text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    kind              text NOT NULL DEFAULT 'login' CHECK (kind IN ('login', 'api_key', 'wifi', 'note', 'other')),
    username          text NOT NULL DEFAULT '' CHECK (length(username) <= 500),
    url               text NOT NULL DEFAULT '' CHECK (length(url) <= 2000),
    -- Not secret: how to use it, who owns it.
    notes             text NOT NULL DEFAULT '' CHECK (length(notes) <= 10000),
    tags              text[] NOT NULL DEFAULT '{}' CHECK (cardinality(tags) <= 20),
    -- The secret, encrypted (purpose vault_item_secret, record id).
    secret_sealed     bytea,
    secret_changed_at timestamptz,
    created_by        uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_by        uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    version           integer NOT NULL DEFAULT 1
);
CREATE INDEX vault_items_org ON vault_items (org_id, lower(name));
CREATE INDEX vault_items_name_trgm ON vault_items USING gin (name gin_trgm_ops);
ALTER TABLE vault_items ENABLE ROW LEVEL SECURITY;
CREATE POLICY vault_items_all ON vault_items FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Who revealed which secret, when, and why. Kept when the item is deleted
-- (item_id becomes NULL), so the trail survives.
CREATE TABLE vault_reveals (
    id           uuid PRIMARY KEY,
    item_id      uuid REFERENCES vault_items (id) ON DELETE SET NULL,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    item_name    text NOT NULL,
    user_id      uuid REFERENCES users (id) ON DELETE SET NULL,
    reason       text NOT NULL DEFAULT '' CHECK (length(reason) <= 500),
    revealed_at  timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX vault_reveals_item ON vault_reveals (item_id, revealed_at DESC);
CREATE INDEX vault_reveals_org ON vault_reveals (org_id, revealed_at DESC);
ALTER TABLE vault_reveals ENABLE ROW LEVEL SECURITY;
CREATE POLICY vault_reveals_all ON vault_reveals FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Technicians keep and use client passwords; read-only users see that they
-- exist. Owners and admins hold `*`.
UPDATE roles SET permissions = permissions || '{vault:read,vault:reveal,vault:manage}'
 WHERE key = 'technician' AND is_system;
UPDATE roles SET permissions = permissions || '{vault:read}'
 WHERE key = 'read_only' AND is_system;
