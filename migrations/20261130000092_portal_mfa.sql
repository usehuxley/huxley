-- Two-factor sign-in for portal accounts (ADR-040): an authenticator app
-- with recovery codes, asked for after a link or password sign-in.

ALTER TABLE portal_accounts
    -- Envelope-encrypted with the company's key. Unconfirmed while
    -- totp_confirmed_at is null.
    ADD COLUMN totp_secret_enc bytea,
    ADD COLUMN totp_confirmed_at timestamptz,
    -- The last code's time step, so a code works once.
    ADD COLUMN totp_last_step bigint;

-- A session whose company requires two-factor sign-in, by someone who
-- hasn't set it up: it can only reach the setup.
ALTER TABLE portal_sessions ADD COLUMN mfa_setup_required boolean NOT NULL DEFAULT false;

CREATE TABLE portal_recovery_codes (
    id          uuid PRIMARY KEY,
    account_id  uuid NOT NULL REFERENCES portal_accounts (id) ON DELETE CASCADE,
    code_hash   bytea NOT NULL,
    used_at     timestamptz,
    UNIQUE (account_id, code_hash)
);

-- Between the first factor and the code. Single use, a few attempts.
CREATE TABLE portal_mfa_challenges (
    id               uuid PRIMARY KEY,
    token_hash       bytea NOT NULL UNIQUE,
    account_id       uuid NOT NULL REFERENCES portal_accounts (id) ON DELETE CASCADE,
    -- How they signed in first: link or password.
    method           text NOT NULL,
    failed_attempts  integer NOT NULL DEFAULT 0,
    expires_at       timestamptz NOT NULL,
    consumed_at      timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX portal_mfa_challenges_expires ON portal_mfa_challenges (expires_at);
