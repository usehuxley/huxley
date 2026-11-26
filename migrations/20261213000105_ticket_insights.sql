-- Ticket insights (design doc §8.8, ADR-050): an AI summary of a ticket,
-- the people, devices, applications and errors it mentions, and a next
-- step, written by the AI ticket insights pack.
CREATE TABLE sd_ticket_insights (
    ticket_id   uuid PRIMARY KEY REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    summary     text NOT NULL CHECK (length(summary) BETWEEN 1 AND 1000),
    -- { people: [], devices: [], applications: [], errors: [] }
    entities    jsonb NOT NULL DEFAULT '{}',
    next_step   text CHECK (next_step IS NULL OR length(next_step) <= 300),
    at          timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE sd_ticket_insights ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_ticket_insights_all ON sd_ticket_insights FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
