-- OAuth 2.1 for MCP clients (ADR-032): Huxley is the authorization server
-- for its own MCP endpoint. Clients register themselves (RFC 7591), people
-- approve them on a consent page, and they get short-lived access tokens
-- that only /mcp accepts, plus rotating refresh tokens.
--
-- Like api_tokens these are system tables (no RLS): they're reached only by
-- the auth service, never by organization-scoped queries.

-- Registered clients. All are public clients (no secret): MCP clients run on
-- people's machines or are browser apps, and PKCE protects the code.
CREATE TABLE oauth_clients (
    id             uuid PRIMARY KEY,
    -- What the client sends as client_id.
    client_id      text NOT NULL UNIQUE,
    -- Self-asserted by the client; shown on the consent page as such.
    name           text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    redirect_uris  text[] NOT NULL CHECK (cardinality(redirect_uris) BETWEEN 1 AND 10),
    created_at     timestamptz NOT NULL DEFAULT now(),
    created_ip     inet
);

-- What a person approved for a client: the access its tokens carry.
CREATE TABLE oauth_grants (
    id                   uuid PRIMARY KEY,
    client_id            uuid NOT NULL REFERENCES oauth_clients (id) ON DELETE CASCADE,
    user_id              uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Permission keys, or {*}: they narrow the person's roles, never widen.
    scopes               text[] NOT NULL CHECK (cardinality(scopes) > 0),
    -- The protected resource (RFC 8707): the MCP endpoint's URL.
    resource             text NOT NULL,
    created_at           timestamptz NOT NULL DEFAULT now(),
    last_used_at         timestamptz,
    last_used_ip         inet,
    revoked_at           timestamptz,
    -- The current refresh token, and the one it replaced: presenting the
    -- replaced one again means it was copied, so the grant is revoked.
    refresh_hash         bytea UNIQUE,
    refresh_expires_at   timestamptz,
    previous_refresh_hash bytea UNIQUE
);
CREATE INDEX oauth_grants_user ON oauth_grants (user_id) WHERE revoked_at IS NULL;
CREATE INDEX oauth_grants_client ON oauth_grants (client_id);

-- Authorization codes: single use, minutes long.
CREATE TABLE oauth_codes (
    code_hash       bytea PRIMARY KEY,
    client_id       uuid NOT NULL REFERENCES oauth_clients (id) ON DELETE CASCADE,
    user_id         uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    redirect_uri    text NOT NULL,
    code_challenge  text NOT NULL,
    scopes          text[] NOT NULL,
    resource        text NOT NULL,
    expires_at      timestamptz NOT NULL,
    -- Set when exchanged; a second exchange revokes the grant it made.
    grant_id        uuid REFERENCES oauth_grants (id) ON DELETE CASCADE
);
CREATE INDEX oauth_codes_expiry ON oauth_codes (expires_at);

-- Access tokens: an hour long, hashed like every other token.
CREATE TABLE oauth_access_tokens (
    token_hash  bytea PRIMARY KEY,
    grant_id    uuid NOT NULL REFERENCES oauth_grants (id) ON DELETE CASCADE,
    expires_at  timestamptz NOT NULL
);
CREATE INDEX oauth_access_tokens_grant ON oauth_access_tokens (grant_id);
CREATE INDEX oauth_access_tokens_expiry ON oauth_access_tokens (expires_at);
