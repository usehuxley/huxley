-- Email verification, password reset, and invitations (design doc §11.2).

ALTER TABLE users ADD COLUMN email_verified_at timestamptz;

-- Users created by an identity provider that verified their address.
UPDATE users u SET email_verified_at = u.created_at
 WHERE EXISTS (SELECT 1 FROM user_identities i WHERE i.user_id = u.id);

CREATE TYPE user_token_purpose AS ENUM ('password_reset', 'invite', 'email_verification');

-- Single-use tokens sent by email. Only the SHA-256 of the 256-bit secret is
-- stored. `email` is the address it was sent to: a verification only counts
-- if the user's address hasn't changed since.
CREATE TABLE user_tokens (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    purpose     user_token_purpose NOT NULL,
    token_hash  bytea NOT NULL UNIQUE,
    email       text NOT NULL,
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX user_tokens_user ON user_tokens (user_id, purpose) WHERE used_at IS NULL;
CREATE INDEX user_tokens_expires ON user_tokens (expires_at);
