-- Calendar sync (ADR-061): people's busy time from their work calendars,
-- so dispatch, the dispatch board and capacity know when they're actually
-- free. Free/busy only by default.

-- How calendars are reached (instance-wide; one row). Microsoft 365 through
-- an msgraph connection at an organization.
CREATE TABLE wf_calendar_settings (
    id             boolean PRIMARY KEY DEFAULT true CHECK (id),
    provider       text NOT NULL CHECK (provider IN ('m365')),
    -- Where the connection is, and its key there.
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    connection     text NOT NULL CHECK (connection ~ '^[a-z][a-z0-9_-]{0,63}$'),
    -- Keep event subjects (shown only to the person and dispatchers).
    keep_subjects  boolean NOT NULL DEFAULT false,
    updated_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_at     timestamptz NOT NULL DEFAULT now()
);

-- Whose calendar is synced, and from where.
CREATE TABLE wf_calendar_people (
    user_id        uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- 'provider' (the settings' provider; their calendar address below, or
    -- their email), 'ics' (a feed URL), or 'none'.
    source         text NOT NULL DEFAULT 'provider' CHECK (source IN ('provider', 'ics', 'none')),
    calendar       text CHECK (calendar IS NULL OR length(calendar) BETWEEN 3 AND 320),
    -- An ICS feed's URL, sealed (it's usually a secret link).
    ics_url        bytea,
    last_synced_at timestamptz,
    last_error     text,
    updated_at     timestamptz NOT NULL DEFAULT now(),
    CHECK (source <> 'ics' OR ics_url IS NOT NULL)
);

-- Busy time, replaced for each person's window on every sync.
CREATE TABLE wf_busy_blocks (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    starts_at   timestamptz NOT NULL,
    ends_at     timestamptz NOT NULL,
    -- busy, tentative, oof (out of office) or elsewhere.
    status      text NOT NULL CHECK (status IN ('busy', 'tentative', 'oof', 'elsewhere')),
    -- Only when the settings keep subjects, and never for private events.
    subject     text CHECK (subject IS NULL OR length(subject) <= 500),
    synced_at   timestamptz NOT NULL DEFAULT now(),
    CHECK (ends_at > starts_at)
);
CREATE INDEX wf_busy_blocks_user ON wf_busy_blocks (user_id, starts_at);
