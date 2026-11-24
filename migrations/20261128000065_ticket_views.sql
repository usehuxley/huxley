-- Saved ticket views (design doc §8.2): a named set of filters on the
-- Tickets list, someone's own or shared with everyone.

CREATE TABLE sd_ticket_views (
    id         uuid PRIMARY KEY,
    -- None: shared.
    owner_id   uuid REFERENCES users (id) ON DELETE CASCADE,
    name       text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 60),
    -- `{"view": "open", "queueId": …, "orgId": …, "search": …, "priority": "p1,p2"}`.
    filters    jsonb NOT NULL DEFAULT '{}'::jsonb,
    version    integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX sd_ticket_views_name
    ON sd_ticket_views (coalesce(owner_id, '00000000-0000-0000-0000-000000000000'::uuid), lower(name));
