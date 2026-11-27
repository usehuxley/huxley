-- SCIM 2.0 provisioning (ADR-088): an identity provider (Entra ID, Okta,
-- JumpCloud…) creates, updates and deactivates Huxley users and manages
-- team membership through /scim/v2. Provisioning hangs off a sign-in
-- provider: each has its own SCIM token, and what it provisioned is linked
-- to it. Teams it manages keep their role assignments, set in Huxley.

ALTER TABLE oidc_providers
    -- SHA-256 of the provider's SCIM bearer token; NULL when SCIM is off.
    ADD COLUMN scim_token_hash bytea UNIQUE,
    ADD COLUMN scim_token_created_at timestamptz,
    ADD COLUMN scim_last_used_at timestamptz;

-- A user the provider provisioned (or adopted by email).
CREATE TABLE scim_users (
    provider_id  uuid NOT NULL REFERENCES oidc_providers (id) ON DELETE CASCADE,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    user_name    text NOT NULL CHECK (length(user_name) BETWEEN 1 AND 320),
    external_id  text CHECK (length(external_id) <= 500),
    -- The SCIM name (givenName, familyName, formatted), as sent.
    name         jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider_id, user_id)
);
CREATE UNIQUE INDEX scim_users_user_name ON scim_users (provider_id, lower(user_name));
CREATE INDEX scim_users_external ON scim_users (provider_id, external_id);
CREATE INDEX scim_users_user ON scim_users (user_id);

-- A team the provider manages (a SCIM group). One provider per team.
CREATE TABLE scim_groups (
    provider_id  uuid NOT NULL REFERENCES oidc_providers (id) ON DELETE CASCADE,
    team_id      uuid NOT NULL UNIQUE REFERENCES teams (id) ON DELETE CASCADE,
    external_id  text CHECK (length(external_id) <= 500),
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (provider_id, team_id)
);
CREATE INDEX scim_groups_external ON scim_groups (provider_id, external_id);
