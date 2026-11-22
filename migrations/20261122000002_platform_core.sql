-- Platform core: application role, organizations, users, sessions, audit log,
-- events outbox. See design doc §4.4, §5, §11, §12.

------------------------------------------------------------------------------
-- Application role
--
-- Migrations run as the connecting (owner) user. At runtime the server runs
-- `SET ROLE huxley_app` on every pooled connection, so Row-Level Security
-- applies even when the connecting user is a superuser or the table owner.
-- This is defense in depth on top of application-level authorization.
------------------------------------------------------------------------------

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'huxley_app') THEN
        CREATE ROLE huxley_app NOLOGIN;
    END IF;
END
$$;

-- Lets a non-superuser connecting user switch to the role.
GRANT huxley_app TO CURRENT_USER;

GRANT USAGE ON SCHEMA public, datasets TO huxley_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO huxley_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT USAGE, SELECT ON SEQUENCES TO huxley_app;

------------------------------------------------------------------------------
-- Organizations (design doc §5)
--
-- `path` is a materialized ltree of ancestor labels, maintained by triggers.
-- Labels are the org id without hyphens. Moving an org (changing parent_id)
-- rewrites the paths of its whole subtree. Tree maintenance must run in
-- system scope so RLS doesn't hide descendants from the trigger.
------------------------------------------------------------------------------

CREATE TYPE org_kind AS ENUM ('provider', 'client');
CREATE TYPE org_status AS ENUM ('active', 'archived');

CREATE TABLE organizations (
    id          uuid PRIMARY KEY,
    parent_id   uuid REFERENCES organizations (id) ON DELETE RESTRICT,
    path        ltree NOT NULL,
    kind        org_kind NOT NULL,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    status      org_status NOT NULL DEFAULT 'active',
    version     integer NOT NULL DEFAULT 1,
    created_at  timestamptz NOT NULL DEFAULT now(),
    created_by  uuid,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    updated_by  uuid,
    -- The root is the owning company and must be a provider.
    CONSTRAINT organizations_root_is_provider CHECK (parent_id IS NOT NULL OR kind = 'provider'),
    CONSTRAINT organizations_not_own_parent CHECK (parent_id IS DISTINCT FROM id)
);

-- Exactly one root per instance.
CREATE UNIQUE INDEX organizations_single_root ON organizations ((true)) WHERE parent_id IS NULL;
CREATE INDEX organizations_path_gist ON organizations USING gist (path);
CREATE INDEX organizations_parent ON organizations (parent_id);
-- Active siblings have distinct names (case-insensitive).
CREATE UNIQUE INDEX organizations_sibling_name
    ON organizations (coalesce(parent_id, '00000000-0000-0000-0000-000000000000'::uuid), lower(name))
    WHERE status = 'active';

CREATE FUNCTION huxley_org_label(id uuid) RETURNS ltree
LANGUAGE sql IMMUTABLE AS $$
    SELECT replace(id::text, '-', '')::ltree
$$;

CREATE FUNCTION huxley_org_set_path() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    parent_path ltree;
    parent_kind org_kind;
BEGIN
    IF NEW.parent_id IS NULL THEN
        NEW.path := huxley_org_label(NEW.id);
        RETURN NEW;
    END IF;

    SELECT path, kind INTO parent_path, parent_kind FROM organizations WHERE id = NEW.parent_id;
    IF parent_path IS NULL THEN
        RAISE EXCEPTION 'parent organization % not found', NEW.parent_id
            USING ERRCODE = 'foreign_key_violation';
    END IF;

    -- Moving under one's own descendant would create a cycle.
    IF TG_OP = 'UPDATE' AND parent_path <@ OLD.path THEN
        RAISE EXCEPTION 'cannot move an organization under its own descendant'
            USING ERRCODE = 'check_violation', CONSTRAINT = 'organizations_no_cycles';
    END IF;

    -- A client's sub-organizations are clients too; providers sit above clients.
    IF parent_kind = 'client' AND NEW.kind = 'provider' THEN
        RAISE EXCEPTION 'a provider organization cannot be placed under a client'
            USING ERRCODE = 'check_violation', CONSTRAINT = 'organizations_provider_under_client';
    END IF;

    NEW.path := parent_path || huxley_org_label(NEW.id);
    RETURN NEW;
END
$$;

CREATE TRIGGER organizations_set_path_insert
    BEFORE INSERT ON organizations
    FOR EACH ROW EXECUTE FUNCTION huxley_org_set_path();

CREATE TRIGGER organizations_set_path_update
    BEFORE UPDATE OF parent_id ON organizations
    FOR EACH ROW WHEN (OLD.parent_id IS DISTINCT FROM NEW.parent_id)
    EXECUTE FUNCTION huxley_org_set_path();

CREATE FUNCTION huxley_org_move_subtree() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    -- The statement below rewrites the entire subtree at once; the per-row
    -- triggers it fires for descendants have nothing left to do.
    IF pg_trigger_depth() > 1 THEN
        RETURN NULL;
    END IF;
    UPDATE organizations
       SET path = NEW.path || subpath(path, nlevel(OLD.path))
     WHERE path <@ OLD.path AND id <> NEW.id;
    RETURN NULL;
END
$$;

-- Keyed on parent_id, not path: a column-list trigger fires only for columns
-- named in the UPDATE's SET clause, and path is changed by the BEFORE trigger.
CREATE TRIGGER organizations_move_subtree
    AFTER UPDATE OF parent_id ON organizations
    FOR EACH ROW WHEN (OLD.path IS DISTINCT FROM NEW.path)
    EXECUTE FUNCTION huxley_org_move_subtree();

CREATE TRIGGER organizations_touch
    BEFORE UPDATE ON organizations
    FOR EACH ROW EXECUTE FUNCTION huxley_touch_updated_at();

ALTER TABLE organizations ENABLE ROW LEVEL SECURITY;

CREATE POLICY organizations_select ON organizations FOR SELECT TO huxley_app
    USING (huxley_org_in_scope(id));
-- New orgs are created under a parent the caller can see.
CREATE POLICY organizations_insert ON organizations FOR INSERT TO huxley_app
    WITH CHECK (huxley_org_in_scope(id) OR huxley_org_in_scope(parent_id));
CREATE POLICY organizations_update ON organizations FOR UPDATE TO huxley_app
    USING (huxley_org_in_scope(id)) WITH CHECK (huxley_org_in_scope(id));
CREATE POLICY organizations_delete ON organizations FOR DELETE TO huxley_app
    USING (huxley_org_in_scope(id));

------------------------------------------------------------------------------
-- Users and sessions (design doc §11.2)
--
-- Staff users belong to the instance, not an org; role assignments (Phase 1,
-- RBAC) scope what they can see.
------------------------------------------------------------------------------

CREATE TYPE user_status AS ENUM ('active', 'disabled');

CREATE TABLE users (
    id                  uuid PRIMARY KEY,
    email               text NOT NULL CHECK (position('@' IN email) > 1 AND length(email) <= 320),
    display_name        text NOT NULL CHECK (length(btrim(display_name)) BETWEEN 1 AND 200),
    status              user_status NOT NULL DEFAULT 'active',
    failed_login_count  integer NOT NULL DEFAULT 0,
    locked_until        timestamptz,
    last_login_at       timestamptz,
    version             integer NOT NULL DEFAULT 1,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX users_email_unique ON users (lower(email));

CREATE TRIGGER users_touch
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION huxley_touch_updated_at();

-- Separate table so password hashes are never selected by accident with users.
CREATE TABLE user_passwords (
    user_id     uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    hash        text NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now()
);

-- Session tokens are random 256-bit values. Only their SHA-256 is stored, so a
-- database leak does not yield usable sessions.
CREATE TABLE sessions (
    id                   uuid PRIMARY KEY,
    user_id              uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash           bytea NOT NULL UNIQUE,
    created_at           timestamptz NOT NULL DEFAULT now(),
    last_seen_at         timestamptz NOT NULL DEFAULT now(),
    idle_expires_at      timestamptz NOT NULL,
    absolute_expires_at  timestamptz NOT NULL,
    ip                   inet,
    user_agent           text,
    revoked_at           timestamptz
);

CREATE INDEX sessions_user ON sessions (user_id) WHERE revoked_at IS NULL;

------------------------------------------------------------------------------
-- Audit log (design doc §12)
--
-- Append-only and hash-chained: each row's hash covers its content and the
-- previous row's hash, so any edit or deletion breaks the chain. The chain
-- head lives in a single-row table; updating it serializes audit writes.
-- `huxley admin verify-audit` recomputes the chain.
------------------------------------------------------------------------------

CREATE TABLE audit_chain_head (
    id    integer PRIMARY KEY CHECK (id = 1),
    seq   bigint NOT NULL,
    hash  bytea NOT NULL
);
INSERT INTO audit_chain_head (id, seq, hash) VALUES (1, 0, '\x0000000000000000000000000000000000000000000000000000000000000000');

CREATE TABLE audit_events (
    id           uuid PRIMARY KEY,
    seq          bigint NOT NULL UNIQUE,
    occurred_at  timestamptz NOT NULL DEFAULT now(),
    actor_type   text NOT NULL CHECK (actor_type IN ('user', 'system', 'anonymous')),
    actor_id     uuid,
    via          text NOT NULL CHECK (via IN ('ui', 'api', 'cli', 'mcp', 'workflow', 'system')),
    ip           inet,
    user_agent   text,
    request_id   text,
    org_id       uuid,
    action       text NOT NULL CHECK (action ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    target_type  text,
    target_id    uuid,
    details      jsonb NOT NULL DEFAULT '{}'::jsonb,
    prev_hash    bytea NOT NULL,
    hash         bytea NOT NULL
);

CREATE INDEX audit_events_occurred ON audit_events (occurred_at DESC);
CREATE INDEX audit_events_org ON audit_events (org_id, occurred_at DESC);
CREATE INDEX audit_events_target ON audit_events (target_type, target_id);
CREATE INDEX audit_events_actor ON audit_events (actor_id, occurred_at DESC);

-- The canonical hash of one audit row. Used both when writing and verifying.
CREATE FUNCTION huxley_audit_hash(prev bytea, e audit_events) RETURNS bytea
LANGUAGE sql STABLE AS $$
    SELECT digest(
        prev || convert_to(concat_ws(E'\x1f',
            e.seq::text,
            (extract(epoch FROM e.occurred_at) * 1000000)::bigint::text,
            e.actor_type,
            coalesce(e.actor_id::text, ''),
            e.via,
            coalesce(host(e.ip), ''),
            coalesce(e.user_agent, ''),
            coalesce(e.request_id, ''),
            coalesce(e.org_id::text, ''),
            e.action,
            coalesce(e.target_type, ''),
            coalesce(e.target_id::text, ''),
            e.details::text
        ), 'UTF8'),
        'sha256')
$$;

CREATE FUNCTION huxley_audit_chain() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    head record;
BEGIN
    UPDATE audit_chain_head SET seq = seq + 1 WHERE id = 1 RETURNING seq, hash INTO head;
    NEW.seq := head.seq;
    NEW.occurred_at := now();
    NEW.prev_hash := head.hash;
    NEW.hash := huxley_audit_hash(head.hash, NEW);
    UPDATE audit_chain_head SET hash = NEW.hash WHERE id = 1;
    RETURN NEW;
END
$$;

CREATE TRIGGER audit_events_chain
    BEFORE INSERT ON audit_events
    FOR EACH ROW EXECUTE FUNCTION huxley_audit_chain();

-- Append-only for the application.
REVOKE UPDATE, DELETE, TRUNCATE ON audit_events FROM huxley_app;
REVOKE DELETE, TRUNCATE ON audit_chain_head FROM huxley_app;

ALTER TABLE audit_events ENABLE ROW LEVEL SECURITY;
CREATE POLICY audit_events_select ON audit_events FOR SELECT TO huxley_app
    USING (org_id IS NULL OR huxley_org_in_scope(org_id));
CREATE POLICY audit_events_insert ON audit_events FOR INSERT TO huxley_app
    WITH CHECK (true);

------------------------------------------------------------------------------
-- Events outbox (design doc §4.3)
--
-- Domain events are written in the same transaction as the change that causes
-- them. The dispatcher (Phase 2) publishes them to event triggers.
------------------------------------------------------------------------------

CREATE TABLE events_outbox (
    id            uuid PRIMARY KEY,
    occurred_at   timestamptz NOT NULL DEFAULT now(),
    org_id        uuid,
    event_type    text NOT NULL CHECK (event_type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    subject_type  text NOT NULL,
    subject_id    uuid NOT NULL,
    payload       jsonb NOT NULL DEFAULT '{}'::jsonb,
    published_at  timestamptz
);

CREATE INDEX events_outbox_unpublished ON events_outbox (occurred_at) WHERE published_at IS NULL;

ALTER TABLE events_outbox ENABLE ROW LEVEL SECURITY;
CREATE POLICY events_outbox_select ON events_outbox FOR SELECT TO huxley_app
    USING (org_id IS NULL OR huxley_org_in_scope(org_id));
CREATE POLICY events_outbox_insert ON events_outbox FOR INSERT TO huxley_app
    WITH CHECK (true);
CREATE POLICY events_outbox_update ON events_outbox FOR UPDATE TO huxley_app
    USING (huxley_org_in_scope(coalesce(org_id, '00000000-0000-0000-0000-000000000000'::uuid)));
