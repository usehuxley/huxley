-- Publishing dashboards (design doc §8.12, ADR-045): a personal dashboard
-- is submitted for an audience (teams, roles, the client portal); someone
-- with dashboard:manage approves it and it's shared with that audience, or
-- rejects it with a comment. Each approval is a numbered publication.

-- Who sees a shared dashboard: members of these teams or holders of these
-- roles (both empty: everyone with dashboard:read there).
ALTER TABLE dashboards ADD COLUMN audience_teams uuid[] NOT NULL DEFAULT '{}';
ALTER TABLE dashboards ADD COLUMN audience_roles uuid[] NOT NULL DEFAULT '{}';

CREATE TABLE dashboard_submissions (
    id             uuid PRIMARY KEY,
    dashboard_id   uuid NOT NULL REFERENCES dashboards (id) ON DELETE CASCADE,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    submitted_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    submitted_at   timestamptz NOT NULL DEFAULT now(),
    -- The audience asked for.
    teams          uuid[] NOT NULL DEFAULT '{}',
    roles          uuid[] NOT NULL DEFAULT '{}',
    portal         boolean NOT NULL DEFAULT false,
    note           text CHECK (note IS NULL OR length(note) <= 1000),
    -- What was reviewed: { name, description, range, widgets }.
    snapshot       jsonb NOT NULL,
    status         text NOT NULL DEFAULT 'pending'
                   CHECK (status IN ('pending', 'approved', 'rejected', 'withdrawn')),
    decided_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    decided_at     timestamptz,
    comment        text CHECK (comment IS NULL OR length(comment) <= 1000),
    -- The publication's number (approved ones: 1, 2, …).
    number         integer,
    CHECK ((status = 'approved') = (number IS NOT NULL))
);
CREATE UNIQUE INDEX dashboard_submissions_pending ON dashboard_submissions (dashboard_id) WHERE status = 'pending';
CREATE INDEX dashboard_submissions_dashboard ON dashboard_submissions (dashboard_id, submitted_at);
ALTER TABLE dashboard_submissions ENABLE ROW LEVEL SECURITY;
CREATE POLICY dashboard_submissions_all ON dashboard_submissions FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
