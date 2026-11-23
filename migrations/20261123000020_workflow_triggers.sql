-- Triggers of published workflows (design doc §6.5), synced on publish.
-- Webhooks keep their public id across versions (keyed by the trigger id in
-- the document), so republishing doesn't change their URL.

CREATE TABLE workflow_triggers (
    id            uuid PRIMARY KEY,
    workflow_id   uuid NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    trigger_id    text NOT NULL,
    type          text NOT NULL,
    config        jsonb NOT NULL,
    -- Webhooks: the unguessable part of /hooks/{public_id}.
    public_id     text UNIQUE,
    -- Schedules: when the scheduler next starts a run.
    next_run_at   timestamptz,
    last_run_at   timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workflow_id, trigger_id)
);

CREATE INDEX workflow_triggers_due ON workflow_triggers (next_run_at) WHERE next_run_at IS NOT NULL;

ALTER TABLE workflow_triggers ENABLE ROW LEVEL SECURITY;
CREATE POLICY workflow_triggers_all ON workflow_triggers FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
