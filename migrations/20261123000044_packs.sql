-- Packs (ADR-028): uploaded packs waiting for review, and what's installed
-- where (with the ids of what each install created, for upgrades).
CREATE TABLE pack_uploads (
    id           uuid PRIMARY KEY,
    uploaded_by  uuid REFERENCES users (id) ON DELETE CASCADE,
    bytes        bytea NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE pack_uploads ENABLE ROW LEVEL SECURITY;
CREATE POLICY pack_uploads_system ON pack_uploads FOR ALL TO huxley_app
    USING (current_setting('huxley.org_scope', true) = '*')
    WITH CHECK (current_setting('huxley.org_scope', true) = '*');

CREATE TABLE pack_installs (
    id            uuid PRIMARY KEY,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    pack_key      text NOT NULL,
    name          text NOT NULL,
    version       text NOT NULL,
    publisher     text NOT NULL DEFAULT '',
    trust         text NOT NULL,
    -- { "integrations": { "<key>": "<id>" }, "workflows": {…}, … }
    items         jsonb NOT NULL,
    installed_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    installed_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, pack_key)
);

ALTER TABLE pack_installs ENABLE ROW LEVEL SECURITY;
CREATE POLICY pack_installs_all ON pack_installs FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
