-- Role-based access control (design doc §11.3).
--
-- Roles bundle permission keys. Assignments grant a role to a user at an
-- organization; with `inherit`, the grant also covers that org's descendants.
-- Instance-wide permissions (managing users, roles, the audit log) are checked
-- against the root organization.

CREATE TABLE roles (
    id           uuid PRIMARY KEY,
    key          text NOT NULL UNIQUE CHECK (key ~ '^[a-z][a-z0-9_]*$'),
    name         text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    description  text NOT NULL DEFAULT '',
    -- System roles ship with Huxley and can't be edited or deleted.
    is_system    boolean NOT NULL DEFAULT false,
    -- Permission keys, or '*' for every permission.
    permissions  text[] NOT NULL DEFAULT '{}',
    version      integer NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TRIGGER roles_touch
    BEFORE UPDATE ON roles
    FOR EACH ROW EXECUTE FUNCTION huxley_touch_updated_at();

INSERT INTO roles (id, key, name, description, is_system, permissions) VALUES
    ('00000000-0000-7000-8000-000000000001', 'owner', 'Owner',
     'Full control of the instance, including granting and removing the Owner role.',
     true, '{*}'),
    ('00000000-0000-7000-8000-000000000002', 'admin', 'Administrator',
     'Manages organizations, users, and role assignments. Cannot grant or remove Owner.',
     true, '{*}'),
    ('00000000-0000-7000-8000-000000000003', 'technician', 'Technician',
     'Sees the organizations and users in their scope.',
     true, '{org:read,user:read}'),
    ('00000000-0000-7000-8000-000000000004', 'read_only', 'Read-only',
     'Views organizations, users, roles, and the audit log without changing anything.',
     true, '{org:read,user:read,role:read,audit:read}');

CREATE TABLE role_assignments (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role_id     uuid NOT NULL REFERENCES roles (id) ON DELETE RESTRICT,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- Also applies to every descendant of org_id.
    inherit     boolean NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL DEFAULT now(),
    created_by  uuid,
    CONSTRAINT role_assignments_unique UNIQUE (user_id, role_id, org_id)
);

CREATE INDEX role_assignments_user ON role_assignments (user_id);
CREATE INDEX role_assignments_org ON role_assignments (org_id);

ALTER TABLE role_assignments ENABLE ROW LEVEL SECURITY;
CREATE POLICY role_assignments_all ON role_assignments FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));

-- Instances bootstrapped before RBAC existed: make the bootstrap user Owner.
INSERT INTO role_assignments (id, user_id, role_id, org_id, inherit)
SELECT gen_random_uuid(), e.target_id, '00000000-0000-7000-8000-000000000001', e.org_id, true
  FROM audit_events e
  JOIN users u ON u.id = e.target_id
  JOIN organizations o ON o.id = e.org_id
 WHERE e.action = 'instance.bootstrapped' AND e.target_type = 'user'
ON CONFLICT DO NOTHING;
