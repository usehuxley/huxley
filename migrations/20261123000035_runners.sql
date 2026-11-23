-- Script runners (design doc §6.9, ADR-025): the instance's runner CA,
-- the runners it knows, and single-use enrolment tokens. Runners belong to
-- the instance, not an organization: only the system scope sees these.

CREATE TABLE runner_ca (
    singleton   boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    -- The organization whose data key seals the CA's key (the root).
    org_id      uuid NOT NULL REFERENCES organizations (id),
    cert_pem    text NOT NULL,
    key_sealed  bytea NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TYPE runner_source AS ENUM ('huxley', 'external');

CREATE TABLE runners (
    id               uuid PRIMARY KEY,
    name             text NOT NULL UNIQUE CHECK (name ~ '^[a-z0-9][a-z0-9.-]{0,62}$'),
    source           runner_source NOT NULL,
    -- The serial of the latest certificate Huxley issued it (huxley only):
    -- older certificates stop working when a new one is issued.
    cert_serial      text,
    cert_expires_at  timestamptz,
    languages        text[] NOT NULL DEFAULT '{}',
    version          text,
    last_seen_at     timestamptz,
    revoked_at       timestamptz,
    created_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at       timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE runner_enrollments (
    id          uuid PRIMARY KEY,
    token_hash  bytea NOT NULL UNIQUE,
    name        text NOT NULL,
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz,
    runner_id   uuid REFERENCES runners (id) ON DELETE SET NULL,
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE runner_ca ENABLE ROW LEVEL SECURITY;
CREATE POLICY runner_ca_system ON runner_ca FOR ALL TO huxley_app
    USING (current_setting('huxley.org_scope', true) = '*')
    WITH CHECK (current_setting('huxley.org_scope', true) = '*');
ALTER TABLE runners ENABLE ROW LEVEL SECURITY;
CREATE POLICY runners_system ON runners FOR ALL TO huxley_app
    USING (current_setting('huxley.org_scope', true) = '*')
    WITH CHECK (current_setting('huxley.org_scope', true) = '*');
ALTER TABLE runner_enrollments ENABLE ROW LEVEL SECURITY;
CREATE POLICY runner_enrollments_system ON runner_enrollments FOR ALL TO huxley_app
    USING (current_setting('huxley.org_scope', true) = '*')
    WITH CHECK (current_setting('huxley.org_scope', true) = '*');
