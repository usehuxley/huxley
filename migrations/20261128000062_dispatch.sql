-- Dispatch (design doc §8.2): how a queue's new tickets are assigned.

ALTER TABLE sd_queues
    ADD COLUMN dispatch text NOT NULL DEFAULT 'manual'
        CHECK (dispatch IN ('manual', 'round_robin', 'load', 'skills'));

-- When each member was last given one of the queue's tickets by dispatch:
-- round-robin goes to whoever has waited longest.
CREATE TABLE sd_dispatch_turns (
    queue_id         uuid NOT NULL REFERENCES sd_queues (id) ON DELETE CASCADE,
    org_id           uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id          uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    last_assigned_at timestamptz NOT NULL,
    PRIMARY KEY (queue_id, user_id)
);
ALTER TABLE sd_dispatch_turns ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_dispatch_turns_all ON sd_dispatch_turns FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
