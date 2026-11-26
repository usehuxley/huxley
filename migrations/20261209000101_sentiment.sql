-- Sentiment (design doc §8.9, ADR-046): each client message's tone, -2 (very
-- negative) to 2 (very positive), with a one-line reason, judged by a
-- workflow (the AI sentiment pack) and recorded here. It's a client health
-- factor, and a falling trend flags the client.

CREATE TABLE cx_sentiment (
    id          uuid PRIMARY KEY,
    -- The client company.
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    ticket_id   uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    -- The message judged (none: the ticket's description).
    note_id     uuid REFERENCES sd_ticket_notes (id) ON DELETE CASCADE,
    contact_id  uuid REFERENCES crm_contacts (id) ON DELETE SET NULL,
    at          timestamptz NOT NULL DEFAULT now(),
    score       smallint NOT NULL CHECK (score BETWEEN -2 AND 2),
    reason      text NOT NULL CHECK (length(reason) BETWEEN 1 AND 300)
);
CREATE UNIQUE INDEX cx_sentiment_note ON cx_sentiment (note_id) WHERE note_id IS NOT NULL;
CREATE UNIQUE INDEX cx_sentiment_description ON cx_sentiment (ticket_id) WHERE note_id IS NULL;
CREATE INDEX cx_sentiment_org ON cx_sentiment (org_id, at);
ALTER TABLE cx_sentiment ENABLE ROW LEVEL SECURITY;
CREATE POLICY cx_sentiment_all ON cx_sentiment FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- The day's snapshot flagged falling sentiment (flagged again only after a
-- week without).
ALTER TABLE cx_health ADD COLUMN sentiment_flagged boolean NOT NULL DEFAULT false;
