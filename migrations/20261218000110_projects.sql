-- Projects (design doc §8.14, ADR-056): a client's project with phases,
-- milestones and tasks. Tasks are tickets of the Project task type (they
-- show in My Work and take time like any ticket). Templates lay a project
-- out; budget burn comes from time entries on its tasks, with threshold
-- events at set percentages.

DO $$
DECLARE
    statuses constant jsonb := '[
        {"key": "to_do", "name": "To do", "category": "new"},
        {"key": "in_progress", "name": "In progress", "category": "open"},
        {"key": "blocked", "name": "Blocked", "category": "pending"},
        {"key": "done", "name": "Done", "category": "resolved"},
        {"key": "closed", "name": "Closed", "category": "closed"}
    ]';
    s jsonb;
    i integer := 0;
    task_type uuid := gen_random_uuid();
BEGIN
    INSERT INTO sd_ticket_types (id, key, name, description, is_system, position)
    VALUES (task_type, 'project_task', 'Project task', 'A task in a project.', true, 5)
    ON CONFLICT (key) DO NOTHING;
    SELECT id INTO task_type FROM sd_ticket_types WHERE key = 'project_task';
    FOR s IN SELECT * FROM jsonb_array_elements(statuses) LOOP
        i := i + 1;
        INSERT INTO sd_statuses (id, type_id, key, name, category, position)
        VALUES (gen_random_uuid(), task_type, s->>'key', s->>'name', s->>'category', i)
        ON CONFLICT (type_id, key) DO NOTHING;
    END LOOP;
END $$;

-- Project tasks come from projects.
ALTER TABLE sd_tickets DROP CONSTRAINT sd_tickets_source_check;
ALTER TABLE sd_tickets ADD CONSTRAINT sd_tickets_source_check
    CHECK (source IN ('web', 'email', 'portal', 'api', 'workflow', 'phone', 'project'));

CREATE TABLE prj_templates (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name         text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    description  text NOT NULL DEFAULT '' CHECK (length(description) <= 2000),
    -- { phases: [{ name, budgetHours, tasks: [{ key, title, description,
    -- estimateHours, dependsOn: [keys], clientVisible }] }],
    -- milestones: [{ name, offsetDays, clientVisible }] }
    structure    jsonb NOT NULL,
    version      integer NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE prj_projects (
    id                uuid PRIMARY KEY,
    -- The client company it's for.
    org_id            uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name              text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    description       text NOT NULL DEFAULT '' CHECK (length(description) <= 5000),
    manager_user_id   uuid REFERENCES users (id) ON DELETE SET NULL,
    status            text NOT NULL DEFAULT 'active'
                      CHECK (status IN ('planned', 'active', 'on_hold', 'completed', 'cancelled')),
    start_on          date NOT NULL,
    end_on            date CHECK (end_on IS NULL OR end_on >= start_on),
    budget_hours      numeric(10, 2) CHECK (budget_hours IS NULL OR budget_hours >= 0),
    billing_method    text NOT NULL DEFAULT 'time_materials'
                      CHECK (billing_method IN ('fixed_fee', 'milestone', 'time_materials', 'not_to_exceed', 'agreement')),
    template_id       uuid REFERENCES prj_templates (id) ON DELETE SET NULL,
    -- Budget-used percentages that raise project.budget_threshold, and
    -- those already raised.
    thresholds        smallint[] NOT NULL DEFAULT '{50,75,90,100}',
    thresholds_fired  smallint[] NOT NULL DEFAULT '{}',
    -- Shown to the client's admins in the portal.
    portal_visible    boolean NOT NULL DEFAULT true,
    created_by        uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    version           integer NOT NULL DEFAULT 1
);
CREATE INDEX prj_projects_org ON prj_projects (org_id);

CREATE TABLE prj_phases (
    id            uuid PRIMARY KEY,
    project_id    uuid NOT NULL REFERENCES prj_projects (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name          text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    position      integer NOT NULL,
    budget_hours  numeric(10, 2) CHECK (budget_hours IS NULL OR budget_hours >= 0)
);
CREATE INDEX prj_phases_project ON prj_phases (project_id, position);

CREATE TABLE prj_milestones (
    id              uuid PRIMARY KEY,
    project_id      uuid NOT NULL REFERENCES prj_projects (id) ON DELETE CASCADE,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name            text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    due_on          date,
    reached_at      timestamptz,
    client_visible  boolean NOT NULL DEFAULT true,
    position        integer NOT NULL
);
CREATE INDEX prj_milestones_project ON prj_milestones (project_id, position);

-- A task's place in its project (the task itself is the ticket).
CREATE TABLE prj_tasks (
    ticket_id       uuid PRIMARY KEY REFERENCES sd_tickets (id) ON DELETE CASCADE,
    project_id      uuid NOT NULL REFERENCES prj_projects (id) ON DELETE CASCADE,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    phase_id        uuid REFERENCES prj_phases (id) ON DELETE SET NULL,
    estimate_hours  numeric(10, 2) CHECK (estimate_hours IS NULL OR estimate_hours >= 0),
    position        integer NOT NULL DEFAULT 0,
    -- Tasks (tickets) that must be done first.
    depends_on      uuid[] NOT NULL DEFAULT '{}',
    client_visible  boolean NOT NULL DEFAULT false
);
CREATE INDEX prj_tasks_project ON prj_tasks (project_id);

ALTER TABLE prj_templates ENABLE ROW LEVEL SECURITY;
ALTER TABLE prj_projects ENABLE ROW LEVEL SECURITY;
ALTER TABLE prj_phases ENABLE ROW LEVEL SECURITY;
ALTER TABLE prj_milestones ENABLE ROW LEVEL SECURITY;
ALTER TABLE prj_tasks ENABLE ROW LEVEL SECURITY;
DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['prj_templates', 'prj_projects', 'prj_phases', 'prj_milestones', 'prj_tasks'] LOOP
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;

-- Who works projects: technicians read and work them (as tickets); admins
-- hold everything already.
UPDATE roles SET permissions = permissions || '{project:read,project:write}'
 WHERE key = 'technician' AND is_system;
UPDATE roles SET permissions = permissions || '{project:read}'
 WHERE key = 'read_only' AND is_system;
