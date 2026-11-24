-- Canned responses (design doc §8.2): shared replies and notes, with
-- placeholders filled in for the ticket and the technician. Instance-wide,
-- like the skills catalog.

CREATE TABLE sd_canned_responses (
    id          uuid PRIMARY KEY,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    category    text NOT NULL DEFAULT '' CHECK (length(category) <= 100),
    -- The kind of note it makes: a reply to the requester, or internal.
    kind        text NOT NULL DEFAULT 'public' CHECK (kind IN ('public', 'internal')),
    body        text NOT NULL CHECK (length(btrim(body)) BETWEEN 1 AND 20000),
    version     integer NOT NULL DEFAULT 1,
    created_at  timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz
);
CREATE UNIQUE INDEX sd_canned_responses_name ON sd_canned_responses (lower(name)) WHERE archived_at IS NULL;
