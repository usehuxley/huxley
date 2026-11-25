-- Ticket templates (design doc §8.2): a common ticket's type, title,
-- description, impact, urgency and queue, to start new tickets from.
-- Instance-wide, like canned responses.

CREATE TABLE sd_ticket_templates (
    id          uuid PRIMARY KEY,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    type_key    text NOT NULL,
    title       text NOT NULL DEFAULT '' CHECK (length(title) <= 300),
    description text NOT NULL DEFAULT '' CHECK (length(description) <= 100000),
    impact      smallint NOT NULL DEFAULT 2 CHECK (impact BETWEEN 1 AND 3),
    urgency     smallint NOT NULL DEFAULT 2 CHECK (urgency BETWEEN 1 AND 3),
    queue_id    uuid REFERENCES sd_queues (id) ON DELETE SET NULL,
    version     integer NOT NULL DEFAULT 1,
    created_at  timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz
);
CREATE UNIQUE INDEX sd_ticket_templates_name ON sd_ticket_templates (lower(name)) WHERE archived_at IS NULL;
