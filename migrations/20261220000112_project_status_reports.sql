-- Project status reports (ADR-059): written by people or drafted by AI on a
-- schedule, optionally shared with the client in the portal.

CREATE TABLE prj_status_reports (
    id             uuid PRIMARY KEY,
    project_id     uuid NOT NULL REFERENCES prj_projects (id) ON DELETE CASCADE,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    period_start   date,
    period_end     date,
    -- Markdown.
    body           text NOT NULL CHECK (length(btrim(body)) BETWEEN 1 AND 20000),
    -- The project's health when it was written.
    health         text NOT NULL CHECK (health IN ('on_track', 'at_risk', 'off_track')),
    client_visible boolean NOT NULL DEFAULT false,
    -- Who wrote it: a person, or e.g. "AI status reports".
    author         text NOT NULL,
    created_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX prj_status_reports_project ON prj_status_reports (project_id, created_at DESC);

ALTER TABLE prj_status_reports ENABLE ROW LEVEL SECURITY;
CREATE POLICY prj_status_reports_all ON prj_status_reports FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
