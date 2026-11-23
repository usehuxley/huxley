-- Tokens of per-organization connections (ADR-027): one partner connection
-- whose token URL takes each client organization's tenant, so one token
-- per resolved token URL.
CREATE TABLE connection_tenant_tokens (
    connection_id   uuid NOT NULL REFERENCES connections (id) ON DELETE CASCADE,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    token_url       text NOT NULL,
    tokens_sealed   bytea NOT NULL,
    expires_at      timestamptz,
    PRIMARY KEY (connection_id, token_url)
);

ALTER TABLE connection_tenant_tokens ENABLE ROW LEVEL SECURITY;
-- Only the engine (system scope) reads or writes these.
CREATE POLICY connection_tenant_tokens_system ON connection_tenant_tokens FOR ALL TO huxley_app
    USING (current_setting('huxley.org_scope', true) = '*')
    WITH CHECK (current_setting('huxley.org_scope', true) = '*');
