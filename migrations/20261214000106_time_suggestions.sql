-- Suggested time entries (design doc §8.8, ADR-051): the AI
-- auto-documentation pack drafts time for the person who resolved a ticket
-- without logging any; they accept it (a real entry) or dismiss it.
CREATE TABLE sd_time_suggestions (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    ticket_id   uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    worked_on   date NOT NULL,
    minutes     integer NOT NULL CHECK (minutes BETWEEN 1 AND 1440),
    notes       text NOT NULL DEFAULT '' CHECK (length(notes) <= 4000),
    status      text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'dismissed')),
    entry_id    uuid REFERENCES sd_time_entries (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX sd_time_suggestions_pending ON sd_time_suggestions (ticket_id, user_id) WHERE status = 'pending';
CREATE INDEX sd_time_suggestions_user ON sd_time_suggestions (user_id) WHERE status = 'pending';
ALTER TABLE sd_time_suggestions ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_time_suggestions_all ON sd_time_suggestions FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
