-- The script library (design doc §6.9): versioned PowerShell and Python
-- scripts with a parameter schema, run by sandboxed runners. Libraries at
-- an organization are shared with the organizations below it.

CREATE TYPE script_language AS ENUM ('pwsh', 'python');

CREATE TABLE scripts (
    id               uuid PRIMARY KEY,
    org_id           uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    key              text NOT NULL CHECK (key ~ '^[a-z][a-z0-9-]{0,63}$'),
    name             text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    description      text CHECK (length(description) <= 2000),
    language         script_language NOT NULL,
    current_version  integer NOT NULL DEFAULT 1,
    archived         boolean NOT NULL DEFAULT false,
    created_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, key)
);

CREATE TABLE script_versions (
    script_id   uuid NOT NULL REFERENCES scripts (id) ON DELETE CASCADE,
    version     integer NOT NULL CHECK (version > 0),
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    source      text NOT NULL CHECK (length(source) <= 262144),
    -- JSON Schema (an object) of the script's parameters.
    parameters  jsonb NOT NULL,
    notes       text CHECK (length(notes) <= 2000),
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (script_id, version)
);

ALTER TABLE scripts ENABLE ROW LEVEL SECURITY;
CREATE POLICY scripts_all ON scripts FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
ALTER TABLE script_versions ENABLE ROW LEVEL SECURITY;
CREATE POLICY script_versions_all ON script_versions FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));

UPDATE roles SET permissions = permissions || '{script:read}'
 WHERE key IN ('technician', 'read_only') AND is_system;
