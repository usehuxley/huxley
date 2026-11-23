-- OIDC single sign-on (design doc §11.2): identity providers, linked
-- identities, in-flight sign-ins, claim → role mappings, and session
-- assurance (whether a session satisfied MFA).

CREATE TYPE oidc_preset AS ENUM ('generic', 'entra', 'google', 'okta', 'authentik', 'keycloak');
-- When an SSO sign-in counts as multi-factor for the MFA policy.
CREATE TYPE oidc_mfa_trust AS ENUM ('never', 'amr', 'always');

CREATE TABLE oidc_providers (
    id                 uuid PRIMARY KEY,
    -- URL-safe identifier used in sign-in links.
    key                text NOT NULL UNIQUE CHECK (key ~ '^[a-z0-9][a-z0-9-]{0,39}$'),
    name               text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    preset             oidc_preset NOT NULL,
    issuer_url         text NOT NULL CHECK (issuer_url ~ '^https?://'),
    client_id          text NOT NULL CHECK (length(client_id) BETWEEN 1 AND 500),
    -- Envelope-encrypted (ADR-018) with the root org's key. Write-only via the API.
    client_secret_enc  bytea,
    scopes             text[] NOT NULL DEFAULT '{openid,email,profile}',
    enabled            boolean NOT NULL DEFAULT true,
    -- Create a Huxley user on first sign-in when none matches.
    jit_provisioning   boolean NOT NULL DEFAULT false,
    -- Email domains this provider may sign in (and link or create users for).
    allowed_domains    text[] NOT NULL DEFAULT '{}',
    -- Trust the email claim without email_verified (e.g. a single Entra tenant
    -- whose admins control every address).
    trust_email        boolean NOT NULL DEFAULT false,
    -- Claim whose values map to roles, e.g. "groups" or "roles".
    role_claim         text,
    mfa_trust          oidc_mfa_trust NOT NULL DEFAULT 'amr',
    version            integer NOT NULL DEFAULT 1,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE TRIGGER oidc_providers_touch
    BEFORE UPDATE ON oidc_providers
    FOR EACH ROW EXECUTE FUNCTION huxley_touch_updated_at();

CREATE TABLE oidc_role_mappings (
    id           uuid PRIMARY KEY,
    provider_id  uuid NOT NULL REFERENCES oidc_providers (id) ON DELETE CASCADE,
    claim_value  text NOT NULL CHECK (length(claim_value) BETWEEN 1 AND 500),
    role_id      uuid NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    inherit      boolean NOT NULL DEFAULT true,
    CONSTRAINT oidc_role_mappings_unique UNIQUE (provider_id, claim_value, role_id, org_id)
);

-- A Huxley user's account at a provider, keyed by the provider's subject.
CREATE TABLE user_identities (
    id             uuid PRIMARY KEY,
    user_id        uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    provider_id    uuid NOT NULL REFERENCES oidc_providers (id) ON DELETE CASCADE,
    subject        text NOT NULL,
    email          text,
    created_at     timestamptz NOT NULL DEFAULT now(),
    last_login_at  timestamptz,
    CONSTRAINT user_identities_subject UNIQUE (provider_id, subject)
);

CREATE INDEX user_identities_user ON user_identities (user_id);

-- Between redirecting to the provider and its callback. Single use.
CREATE TABLE oidc_login_states (
    id             uuid PRIMARY KEY,
    token_hash     bytea NOT NULL UNIQUE,
    provider_id    uuid NOT NULL REFERENCES oidc_providers (id) ON DELETE CASCADE,
    nonce          text NOT NULL,
    pkce_verifier  text NOT NULL,
    next           text,
    expires_at     timestamptz NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX oidc_login_states_expires ON oidc_login_states (expires_at);

-- Assignments created from a provider's role mappings. Sign-in syncs these
-- (adding and removing) and never touches assignments made by hand.
ALTER TABLE role_assignments
    ADD COLUMN oidc_provider_id uuid REFERENCES oidc_providers (id) ON DELETE CASCADE;

-- How a session signed in, and whether that counted as multi-factor.
ALTER TABLE sessions
    ADD COLUMN auth_method text,
    ADD COLUMN mfa boolean NOT NULL DEFAULT false;
