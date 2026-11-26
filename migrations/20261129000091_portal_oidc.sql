-- Client contacts signing in to the portal with their own company's
-- identity provider (ADR-040): e.g. the client's Entra tenant, matched by
-- the email domains it signs in.

CREATE TABLE portal_identity_providers (
    id                 uuid PRIMARY KEY,
    -- The company it signs in for (and those below it).
    org_id             uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name               text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    preset             text NOT NULL DEFAULT 'entra' CHECK (preset IN ('entra', 'generic')),
    issuer_url         text NOT NULL CHECK (issuer_url ~ '^https?://'),
    client_id          text NOT NULL CHECK (length(client_id) BETWEEN 1 AND 500),
    -- Envelope-encrypted with the company's key; write-only.
    client_secret_enc  bytea,
    -- Email domains it signs in (lowercase); unique across providers.
    email_domains      text[] NOT NULL CHECK (cardinality(email_domains) > 0),
    -- Trust the email claim without email_verified (a tenant whose admins
    -- control every address, as Entra's are).
    trust_email        boolean NOT NULL DEFAULT true,
    -- Give contacts with a matching address an account on first sign-in.
    provision          boolean NOT NULL DEFAULT false,
    default_role       text NOT NULL DEFAULT 'client_user'
                       CHECK (default_role IN ('client_user', 'client_approver', 'client_admin')),
    enabled            boolean NOT NULL DEFAULT true,
    version            integer NOT NULL DEFAULT 1,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX portal_identity_providers_org ON portal_identity_providers (org_id);
ALTER TABLE portal_identity_providers ENABLE ROW LEVEL SECURITY;
CREATE POLICY portal_identity_providers_all ON portal_identity_providers FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- A portal account's identity at a provider.
CREATE TABLE portal_identities (
    id             uuid PRIMARY KEY,
    account_id     uuid NOT NULL REFERENCES portal_accounts (id) ON DELETE CASCADE,
    provider_id    uuid NOT NULL REFERENCES portal_identity_providers (id) ON DELETE CASCADE,
    subject        text NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    last_login_at  timestamptz,
    UNIQUE (provider_id, subject)
);
CREATE INDEX portal_identities_account ON portal_identities (account_id);

-- Between the redirect to the provider and its callback. Single use.
CREATE TABLE portal_oidc_states (
    id             uuid PRIMARY KEY,
    token_hash     bytea NOT NULL UNIQUE,
    provider_id    uuid NOT NULL REFERENCES portal_identity_providers (id) ON DELETE CASCADE,
    nonce          text NOT NULL,
    pkce_verifier  text NOT NULL,
    redirect_uri   text NOT NULL,
    expires_at     timestamptz NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX portal_oidc_states_expires ON portal_oidc_states (expires_at);
