-- Integration poll triggers (design doc §6.3): what each trigger has seen,
-- so only new (or changed) items start runs.
ALTER TABLE workflow_triggers ADD COLUMN last_error text;

CREATE TABLE poll_items (
    trigger_id  uuid NOT NULL REFERENCES workflow_triggers (id) ON DELETE CASCADE,
    item_key    text NOT NULL,
    -- SHA-256 of the item's JSON, to notice changes.
    hash        bytea NOT NULL,
    first_seen  timestamptz NOT NULL DEFAULT now(),
    last_seen   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (trigger_id, item_key)
);

-- Only the scheduler touches these, as the system.
ALTER TABLE poll_items ENABLE ROW LEVEL SECURITY;
CREATE POLICY poll_items_system ON poll_items FOR ALL TO huxley_app
    USING (current_setting('huxley.org_scope', true) = '*')
    WITH CHECK (current_setting('huxley.org_scope', true) = '*');
