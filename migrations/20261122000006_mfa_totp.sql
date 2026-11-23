-- Multi-factor authentication: TOTP authenticator apps, recovery codes, and
-- the short-lived challenges that sit between password and second factor.
-- See design doc §11.2.

-- One authenticator per user. A row with confirmed_at NULL is an enrollment
-- in progress: the user has been shown the secret but hasn't proven they
-- saved it. The secret is envelope-encrypted (ADR-018) with the root org's key.
CREATE TABLE user_totp (
    user_id         uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    secret_enc      bytea NOT NULL,
    confirmed_at    timestamptz,
    -- Highest 30-second time step accepted so far; a code can't be replayed.
    last_used_step  bigint,
    created_at      timestamptz NOT NULL DEFAULT now()
);

-- Single-use codes for when the authenticator is lost. Random and high
-- entropy, so a SHA-256 is stored rather than a slow hash.
CREATE TABLE user_recovery_codes (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    code_hash   bytea NOT NULL UNIQUE,
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX user_recovery_codes_user ON user_recovery_codes (user_id) WHERE used_at IS NULL;

-- Issued after a correct password when the user has MFA. The token is a
-- 256-bit random value; only its SHA-256 is stored.
CREATE TABLE mfa_challenges (
    id               uuid PRIMARY KEY,
    user_id          uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash       bytea NOT NULL UNIQUE,
    expires_at       timestamptz NOT NULL,
    failed_attempts  integer NOT NULL DEFAULT 0,
    consumed_at      timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX mfa_challenges_user ON mfa_challenges (user_id) WHERE consumed_at IS NULL;
