-- Organization variables (design doc §6.6): typed values that workflows read
-- as `vars.<key>`, inherited down the org tree, with lock. Secret variables
-- are encrypted (huxley-crypto) and read only as `secrets.<key>`, so
-- expressions that use them are taint-tracked and redacted.

CREATE TYPE variable_type AS ENUM ('string', 'number', 'bool', 'json', 'secret');

CREATE TABLE org_variables (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    key            text NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]{0,63}$'),
    type           variable_type NOT NULL,
    value          jsonb,
    secret_sealed  bytea,
    description    text CHECK (length(description) <= 500),
    locked         boolean NOT NULL DEFAULT false,
    updated_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, key),
    CHECK ((type = 'secret') = (secret_sealed IS NOT NULL)),
    CHECK ((type = 'secret') = (value IS NULL))
);

ALTER TABLE org_variables ENABLE ROW LEVEL SECURITY;
CREATE POLICY org_variables_all ON org_variables FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
