-- Passkeys (WebAuthn): a second factor after the password, and passwordless
-- sign-in with discoverable credentials. See design doc §11.2.

-- A registered credential. `passkey` is webauthn-rs's serialized Passkey: the
-- public key, algorithm, and signature counter. Nothing in it is secret.
CREATE TABLE user_passkeys (
    id             uuid PRIMARY KEY,
    user_id        uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- A credential belongs to exactly one account (WebAuthn §7.1 step 22).
    credential_id  bytea NOT NULL UNIQUE,
    name           text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    passkey        jsonb NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    last_used_at   timestamptz
);

CREATE INDEX user_passkeys_user ON user_passkeys (user_id);

-- Server-side state between the two halves of a WebAuthn ceremony (it holds
-- the challenge). Single use: finishing a ceremony deletes its row.
CREATE TYPE webauthn_ceremony_kind AS ENUM ('registration', 'mfa', 'login');

CREATE TABLE webauthn_ceremonies (
    id                uuid PRIMARY KEY,
    kind              webauthn_ceremony_kind NOT NULL,
    user_id           uuid REFERENCES users (id) ON DELETE CASCADE,
    mfa_challenge_id  uuid REFERENCES mfa_challenges (id) ON DELETE CASCADE,
    state             jsonb NOT NULL,
    expires_at        timestamptz NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    -- Passwordless sign-in doesn't know the user until the credential answers.
    CONSTRAINT webauthn_ceremonies_user CHECK ((kind = 'login') = (user_id IS NULL)),
    CONSTRAINT webauthn_ceremonies_mfa CHECK ((kind = 'mfa') = (mfa_challenge_id IS NOT NULL))
);

CREATE INDEX webauthn_ceremonies_expires ON webauthn_ceremonies (expires_at);
CREATE INDEX mfa_challenges_expires ON mfa_challenges (expires_at);
