-- OAuth 2.0 for connections (design doc §7.2). Tokens are encrypted apart
-- from the credentials people enter, so refreshing never rewrites those.
ALTER TABLE connections
    ADD COLUMN tokens_sealed bytea,
    ADD COLUMN token_expires_at timestamptz,
    ADD COLUMN authorized_by uuid REFERENCES users (id) ON DELETE SET NULL,
    ADD COLUMN authorized_at timestamptz;

-- A sign-in in progress: the `state` sent to the provider, the PKCE
-- verifier, and who started it. Used once, within ten minutes.
CREATE TABLE connection_sign_ins (
    state          text PRIMARY KEY,
    connection_id  uuid NOT NULL REFERENCES connections (id) ON DELETE CASCADE,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id        uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    code_verifier  text NOT NULL,
    redirect_uri   text NOT NULL,
    expires_at     timestamptz NOT NULL
);

ALTER TABLE connection_sign_ins ENABLE ROW LEVEL SECURITY;
CREATE POLICY connection_sign_ins_all ON connection_sign_ins FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
