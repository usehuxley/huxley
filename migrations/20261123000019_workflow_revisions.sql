-- Optimistic concurrency for workflow drafts: saving names the revision it
-- was based on, so two editors don't silently overwrite each other.
ALTER TABLE workflows ADD COLUMN revision integer NOT NULL DEFAULT 1;

-- Workflow permissions for the built-in roles (owner and admin hold `*`).
UPDATE roles SET permissions = permissions || '{workflow:read,workflow:run}'
 WHERE key = 'technician' AND is_system;
UPDATE roles SET permissions = permissions || '{workflow:read}'
 WHERE key = 'read_only' AND is_system;

-- Starting or cancelling a run enqueues a job inside the user's own
-- transaction (so the job exists exactly when the run does), and
-- `INSERT … ON CONFLICT` needs the row visible, so jobs follow the usual org
-- scope: workers (system scope) see all of them, a user's transaction only
-- those for runs in their orgs. Jobs hold ids, not data.
DROP POLICY jobs_system ON jobs;
CREATE POLICY jobs_all ON jobs FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
