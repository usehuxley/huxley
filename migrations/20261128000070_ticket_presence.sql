-- Who has a ticket open, and who's typing on it (design doc §8.2: the
-- collision indicator). Throwaway: unlogged, and rows older than a minute
-- don't count. Access is checked on the ticket before it's touched.
CREATE UNLOGGED TABLE sd_ticket_presence (
    ticket_id uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    user_id   uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    typing    boolean NOT NULL DEFAULT false,
    seen_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (ticket_id, user_id)
);
CREATE INDEX sd_ticket_presence_seen ON sd_ticket_presence (seen_at);
