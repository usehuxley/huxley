-- Script tasks (design doc §6.9): a script step's work, waiting for a
-- runner. The inputs are encrypted (they may hold secrets); runners get
-- them in the clear over the mTLS gateway.
CREATE TYPE script_task_status AS ENUM ('queued', 'running', 'succeeded', 'failed');

CREATE TABLE script_tasks (
    id              uuid PRIMARY KEY,
    run_id          uuid NOT NULL,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    step_key        text NOT NULL,
    attempt         integer NOT NULL,
    script_id       uuid NOT NULL REFERENCES scripts (id) ON DELETE CASCADE,
    script_version  integer NOT NULL,
    language        script_language NOT NULL,
    inputs_sealed   bytea NOT NULL,
    timeout_secs    integer NOT NULL,
    status          script_task_status NOT NULL DEFAULT 'queued',
    runner_id       uuid REFERENCES runners (id) ON DELETE SET NULL,
    claimed_at      timestamptz,
    output          jsonb,
    error           text,
    exit_code       integer,
    logs            text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    finished_at     timestamptz,
    UNIQUE (run_id, step_key, attempt)
);

CREATE INDEX script_tasks_queue ON script_tasks (language, created_at) WHERE status = 'queued';

ALTER TABLE script_tasks ENABLE ROW LEVEL SECURITY;
CREATE POLICY script_tasks_all ON script_tasks FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
