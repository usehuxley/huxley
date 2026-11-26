-- The client portal's own realm (ADR-040): accounts for client contacts,
-- their sessions, and single-use sign-in links. Not users, not staff
-- sessions.

-- Never used: portal accounts aren't users.
ALTER TABLE crm_contacts DROP COLUMN portal_user_id;

CREATE TABLE portal_accounts (
    id               uuid PRIMARY KEY,
    contact_id       uuid NOT NULL UNIQUE REFERENCES crm_contacts (id) ON DELETE CASCADE,
    -- The contact's company (kept in step with the contact's primary org).
    org_id           uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    email            text NOT NULL CHECK (position('@' IN email) > 1 AND length(email) <= 320),
    role             text NOT NULL DEFAULT 'client_user'
                     CHECK (role IN ('client_user', 'client_approver', 'client_admin')),
    status           text NOT NULL DEFAULT 'invited'
                     CHECK (status IN ('invited', 'active', 'disabled')),
    password_hash    text,
    failed_attempts  integer NOT NULL DEFAULT 0,
    locked_until     timestamptz,
    last_sign_in_at  timestamptz,
    invited_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    version          integer NOT NULL DEFAULT 1,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX portal_accounts_email ON portal_accounts (lower(email));
CREATE INDEX portal_accounts_org ON portal_accounts (org_id);
ALTER TABLE portal_accounts ENABLE ROW LEVEL SECURITY;
CREATE POLICY portal_accounts_all ON portal_accounts FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

CREATE TABLE portal_sessions (
    id                   uuid PRIMARY KEY,
    account_id           uuid NOT NULL REFERENCES portal_accounts (id) ON DELETE CASCADE,
    token_hash           bytea NOT NULL UNIQUE,
    created_at           timestamptz NOT NULL DEFAULT now(),
    last_seen_at         timestamptz NOT NULL DEFAULT now(),
    idle_expires_at      timestamptz NOT NULL,
    absolute_expires_at  timestamptz NOT NULL,
    ip                   inet,
    user_agent           text,
    revoked_at           timestamptz
);
CREATE INDEX portal_sessions_account ON portal_sessions (account_id) WHERE revoked_at IS NULL;

-- Single-use sign-in links (only their hashes).
CREATE TABLE portal_login_tokens (
    id          uuid PRIMARY KEY,
    account_id  uuid NOT NULL REFERENCES portal_accounts (id) ON DELETE CASCADE,
    token_hash  bytea NOT NULL UNIQUE,
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX portal_login_tokens_account ON portal_login_tokens (account_id, created_at);
