-- Calendar sync, part two (ADR-062): appointments written to people's
-- calendars, and conflicts with them reported once.

ALTER TABLE wf_calendar_settings ADD COLUMN write_back boolean NOT NULL DEFAULT false;

-- Events Huxley wrote for appointments. No foreign key: a cancelled
-- appointment's event still has to be removed from the calendar.
CREATE TABLE wf_calendar_writes (
    appointment_id uuid PRIMARY KEY,
    user_id        uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- The calendar it's in, and its id there.
    address        text NOT NULL,
    event_id       text NOT NULL,
    starts_at      timestamptz NOT NULL,
    ends_at        timestamptz NOT NULL,
    written_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX wf_calendar_writes_user ON wf_calendar_writes (user_id, starts_at);

-- Busy time found clashing with an appointment: reported once each.
CREATE TABLE wf_calendar_conflicts (
    appointment_id uuid NOT NULL,
    starts_at      timestamptz NOT NULL,
    ends_at        timestamptz NOT NULL,
    found_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (appointment_id, starts_at, ends_at)
);
