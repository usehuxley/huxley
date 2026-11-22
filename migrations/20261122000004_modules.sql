-- Feature module toggles (design doc §8.0, ADR-017).
--
-- Rows record the desired state. A module is active when it is enabled here
-- and released in the running Huxley version. Rows are keyed by org so a
-- provider org can override the instance default (the root org's rows) later.

CREATE TABLE module_settings (
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    module_key  text NOT NULL CHECK (module_key ~ '^[a-z][a-z0-9_]*$'),
    enabled     boolean NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    updated_by  uuid,
    PRIMARY KEY (org_id, module_key)
);

ALTER TABLE module_settings ENABLE ROW LEVEL SECURITY;
CREATE POLICY module_settings_all ON module_settings FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
