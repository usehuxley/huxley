-- Organization settings (design doc §5.2-5.3): a value set at an org applies
-- to it and every descendant, nearest wins, unless an ancestor locked the key.
-- Keys and value types live in the Rust registry (huxley-orgs::settings).

CREATE TABLE org_settings (
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    key         text NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    value       jsonb NOT NULL,
    -- Descendants can't override a locked value.
    locked      boolean NOT NULL DEFAULT false,
    updated_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, key)
);

CREATE INDEX org_settings_key ON org_settings (key);

ALTER TABLE org_settings ENABLE ROW LEVEL SECURITY;
CREATE POLICY org_settings_all ON org_settings FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
