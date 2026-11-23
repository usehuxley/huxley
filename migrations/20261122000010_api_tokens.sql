-- API tokens and service accounts (design doc §11.1, §11.2).
--
-- A service account is a user with kind = 'service': it holds roles and
-- appears in the audit log like anyone else, but can't sign in
-- interactively; it only uses API tokens. Its email is a synthetic
-- `<id>@service.invalid` address (RFC 2606), so the users table needn't change
-- shape.

CREATE TYPE user_kind AS ENUM ('human', 'service');
ALTER TABLE users ADD COLUMN kind user_kind NOT NULL DEFAULT 'human';
ALTER TABLE users ADD COLUMN description text NOT NULL DEFAULT ''
    CHECK (length(description) <= 500);

-- Bearer tokens. Only the SHA-256 of the 256-bit secret is stored. `scopes`
-- narrows the holder's permissions ('*' = all of them); it never adds any.
CREATE TABLE api_tokens (
    id            uuid PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name          text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    token_hash    bytea NOT NULL UNIQUE,
    -- The first characters of the secret, so people can tell tokens apart.
    hint          text NOT NULL,
    scopes        text[] NOT NULL CHECK (cardinality(scopes) > 0),
    expires_at    timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    last_used_at  timestamptz,
    last_used_ip  inet,
    revoked_at    timestamptz
);

CREATE INDEX api_tokens_user ON api_tokens (user_id) WHERE revoked_at IS NULL;
