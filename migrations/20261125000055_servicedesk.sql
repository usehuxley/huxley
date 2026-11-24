-- The `servicedesk` module's core (design doc §8.2, ADR-036): ticket types
-- with their statuses, the priority matrix, service queues, tickets, notes
-- and each ticket's activity.

-- Configuration: instance-wide, readable by everyone (like roles).
CREATE TABLE sd_ticket_types (
    id          uuid PRIMARY KEY,
    key         text NOT NULL UNIQUE CHECK (key ~ '^[a-z][a-z0-9_]{0,49}$'),
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    description text NOT NULL DEFAULT '',
    is_system   boolean NOT NULL DEFAULT false,
    position    integer NOT NULL DEFAULT 0,
    archived_at timestamptz
);

CREATE TABLE sd_statuses (
    id          uuid PRIMARY KEY,
    type_id     uuid NOT NULL REFERENCES sd_ticket_types (id) ON DELETE CASCADE,
    key         text NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]{0,49}$'),
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    -- What SLAs, views and reports reason about.
    category    text NOT NULL CHECK (category IN ('new', 'open', 'pending', 'resolved', 'closed')),
    -- The SLA clock stops while a ticket is here (e.g. waiting on the client).
    pauses_sla  boolean NOT NULL DEFAULT false,
    -- Needed to enter it: `resolution` (a resolution note), `assignee`.
    requires    text[] NOT NULL DEFAULT '{}',
    position    integer NOT NULL DEFAULT 0,
    UNIQUE (type_id, key)
);

-- Allowed status changes; a type without rows allows any.
CREATE TABLE sd_transitions (
    type_id         uuid NOT NULL REFERENCES sd_ticket_types (id) ON DELETE CASCADE,
    from_status_id  uuid NOT NULL REFERENCES sd_statuses (id) ON DELETE CASCADE,
    to_status_id    uuid NOT NULL REFERENCES sd_statuses (id) ON DELETE CASCADE,
    PRIMARY KEY (from_status_id, to_status_id)
);

CREATE TABLE sd_priority_matrix (
    impact    smallint NOT NULL CHECK (impact BETWEEN 1 AND 3),
    urgency   smallint NOT NULL CHECK (urgency BETWEEN 1 AND 3),
    priority  text NOT NULL CHECK (priority IN ('p1', 'p2', 'p3', 'p4')),
    PRIMARY KEY (impact, urgency)
);
INSERT INTO sd_priority_matrix (impact, urgency, priority) VALUES
    (1, 1, 'p1'), (1, 2, 'p2'), (1, 3, 'p3'),
    (2, 1, 'p2'), (2, 2, 'p3'), (2, 3, 'p4'),
    (3, 1, 'p3'), (3, 2, 'p4'), (3, 3, 'p4');

-- The seeded types. Incident and service request share a workflow; problem
-- and change get theirs with their modules (they start with the same one).
DO $$
DECLARE
    t record;
    statuses constant jsonb := '[
        {"key": "new", "name": "New", "category": "new"},
        {"key": "in_progress", "name": "In progress", "category": "open"},
        {"key": "waiting_client", "name": "Waiting on client", "category": "pending", "pauses": true},
        {"key": "waiting_vendor", "name": "Waiting on third party", "category": "pending", "pauses": true},
        {"key": "resolved", "name": "Resolved", "category": "resolved", "requires": ["resolution"]},
        {"key": "closed", "name": "Closed", "category": "closed"}
    ]';
    s jsonb;
    i integer;
BEGIN
    FOR t IN SELECT * FROM (VALUES
        ('incident', 'Incident', 'Something is broken or degraded.', 1),
        ('service_request', 'Service request', 'Someone needs something: access, a change, information.', 2),
        ('problem', 'Problem', 'The cause behind one or more incidents.', 3),
        ('change', 'Change', 'A planned change to a service.', 4)
    ) AS v(key, name, description, position) LOOP
        INSERT INTO sd_ticket_types (id, key, name, description, is_system, position)
        VALUES (gen_random_uuid(), t.key, t.name, t.description, true, t.position);
        i := 0;
        FOR s IN SELECT * FROM jsonb_array_elements(statuses) LOOP
            i := i + 1;
            INSERT INTO sd_statuses (id, type_id, key, name, category, pauses_sla, requires, position)
            SELECT gen_random_uuid(), id, s->>'key', s->>'name', s->>'category',
                   coalesce((s->>'pauses')::boolean, false),
                   coalesce(ARRAY(SELECT jsonb_array_elements_text(s->'requires')), '{}'),
                   i
              FROM sd_ticket_types WHERE key = t.key;
        END LOOP;
    END LOOP;
END $$;

-- Records: they belong to organizations.
CREATE TABLE sd_queues (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    description text NOT NULL DEFAULT '' CHECK (length(description) <= 2000),
    version     integer NOT NULL DEFAULT 1,
    created_at  timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz
);
CREATE UNIQUE INDEX sd_queues_name ON sd_queues (org_id, lower(name)) WHERE archived_at IS NULL;

CREATE TABLE sd_queue_members (
    queue_id  uuid NOT NULL REFERENCES sd_queues (id) ON DELETE CASCADE,
    org_id    uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id   uuid REFERENCES users (id) ON DELETE CASCADE,
    team_id   uuid REFERENCES teams (id) ON DELETE CASCADE,
    CHECK ((user_id IS NULL) <> (team_id IS NULL))
);
CREATE UNIQUE INDEX sd_queue_members_one ON sd_queue_members (queue_id, coalesce(user_id, team_id));

CREATE SEQUENCE sd_ticket_number START 1001;

CREATE TABLE sd_tickets (
    id                 uuid PRIMARY KEY,
    -- The organization the ticket is for (usually a client).
    org_id             uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    number             bigint NOT NULL UNIQUE DEFAULT nextval('sd_ticket_number'),
    -- The organization's ticket prefix when it was created: numbers don't change.
    number_prefix      text NOT NULL DEFAULT 'T',
    type_id            uuid NOT NULL REFERENCES sd_ticket_types (id),
    status_id          uuid NOT NULL REFERENCES sd_statuses (id),
    title              text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 300),
    description        text NOT NULL DEFAULT '' CHECK (length(description) <= 100000),
    impact             smallint NOT NULL DEFAULT 2 CHECK (impact BETWEEN 1 AND 3),
    urgency            smallint NOT NULL DEFAULT 2 CHECK (urgency BETWEEN 1 AND 3),
    priority           text NOT NULL DEFAULT 'p3' CHECK (priority IN ('p1', 'p2', 'p3', 'p4')),
    -- Set when someone chose the priority instead of the matrix.
    priority_overridden boolean NOT NULL DEFAULT false,
    queue_id           uuid REFERENCES sd_queues (id) ON DELETE SET NULL,
    requester_id       uuid REFERENCES crm_contacts (id) ON DELETE SET NULL,
    assignee_user_id   uuid REFERENCES users (id) ON DELETE SET NULL,
    assignee_team_id   uuid REFERENCES teams (id) ON DELETE SET NULL,
    source             text NOT NULL DEFAULT 'web'
                       CHECK (source IN ('web', 'email', 'portal', 'api', 'workflow', 'phone')),
    custom_fields      jsonb NOT NULL DEFAULT '{}'::jsonb,
    version            integer NOT NULL DEFAULT 1,
    created_at         timestamptz NOT NULL DEFAULT now(),
    created_by         uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_at         timestamptz NOT NULL DEFAULT now(),
    first_response_at  timestamptz,
    resolved_at        timestamptz,
    closed_at          timestamptz
);
CREATE INDEX sd_tickets_org ON sd_tickets (org_id, created_at DESC);
CREATE INDEX sd_tickets_queue ON sd_tickets (queue_id) WHERE closed_at IS NULL;
CREATE INDEX sd_tickets_assignee ON sd_tickets (assignee_user_id) WHERE closed_at IS NULL;
CREATE INDEX sd_tickets_requester ON sd_tickets (requester_id);

CREATE TABLE sd_ticket_notes (
    id                uuid PRIMARY KEY,
    ticket_id         uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id            uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    kind              text NOT NULL CHECK (kind IN ('public', 'internal', 'resolution')),
    body              text NOT NULL CHECK (length(btrim(body)) BETWEEN 1 AND 100000),
    author_user_id    uuid REFERENCES users (id) ON DELETE SET NULL,
    author_contact_id uuid REFERENCES crm_contacts (id) ON DELETE SET NULL,
    created_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sd_ticket_notes_ticket ON sd_ticket_notes (ticket_id, created_at);

CREATE TABLE sd_ticket_activity (
    id         uuid PRIMARY KEY,
    ticket_id  uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id     uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    at         timestamptz NOT NULL DEFAULT now(),
    actor_id   uuid REFERENCES users (id) ON DELETE SET NULL,
    -- created, updated, status, assigned, note
    kind       text NOT NULL,
    details    jsonb NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX sd_ticket_activity_ticket ON sd_ticket_activity (ticket_id, at);

ALTER TABLE sd_queues ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_queues_all ON sd_queues FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
ALTER TABLE sd_queue_members ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_queue_members_all ON sd_queue_members FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
ALTER TABLE sd_tickets ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_tickets_all ON sd_tickets FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
ALTER TABLE sd_ticket_notes ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_ticket_notes_all ON sd_ticket_notes FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
ALTER TABLE sd_ticket_activity ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_ticket_activity_all ON sd_ticket_activity FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- Technicians work tickets; read-only users see them. (Owners and admins hold `*`.)
UPDATE roles SET permissions = permissions || '{ticket:read,ticket:write}'
 WHERE key = 'technician' AND is_system;
UPDATE roles SET permissions = permissions || '{ticket:read}'
 WHERE key = 'read_only' AND is_system;
