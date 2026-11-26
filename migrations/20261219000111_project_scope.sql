-- Projects, part two (ADR-057): task dates for the board and timeline, the
-- agreement a project bills against, and scope change requests the
-- client approves.

ALTER TABLE prj_tasks
    ADD COLUMN start_on date,
    ADD COLUMN due_on date,
    ADD CONSTRAINT prj_tasks_dates CHECK (due_on IS NULL OR start_on IS NULL OR due_on >= start_on);

ALTER TABLE prj_projects
    ADD COLUMN agreement_id uuid REFERENCES contract_agreements (id) ON DELETE SET NULL;

CREATE TABLE prj_scope_changes (
    id            uuid PRIMARY KEY,
    project_id    uuid NOT NULL REFERENCES prj_projects (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    number        integer NOT NULL,
    title         text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
    description   text NOT NULL DEFAULT '' CHECK (length(description) <= 20000),
    -- Hours it adds to the budget (negative: removes).
    hours         numeric(10, 2) NOT NULL DEFAULT 0 CHECK (hours BETWEEN -100000 AND 100000),
    -- What it costs the client, in the agreement's currency, if priced.
    amount        numeric(14, 2) CHECK (amount IS NULL OR amount BETWEEN -100000000 AND 100000000),
    state         text NOT NULL DEFAULT 'draft'
                  CHECK (state IN ('draft', 'submitted', 'approved', 'rejected', 'withdrawn')),
    submitted_at  timestamptz,
    decided_at    timestamptz,
    decided_by    text,
    comment       text,
    created_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    version       integer NOT NULL DEFAULT 1,
    UNIQUE (project_id, number)
);
CREATE INDEX prj_scope_changes_project ON prj_scope_changes (project_id, number);

ALTER TABLE prj_scope_changes ENABLE ROW LEVEL SECURITY;
CREATE POLICY prj_scope_changes_all ON prj_scope_changes FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
