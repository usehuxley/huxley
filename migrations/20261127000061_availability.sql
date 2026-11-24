-- People's availability (design doc §8.15): working schedules and time off.

-- When someone works; none: no schedule (always counted as available).
CREATE TABLE wf_schedules (
    user_id    uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    timezone   text NOT NULL,
    -- `{"mon": [["09:00", "17:00"]], …}`, like a site's opening hours.
    hours      jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE wf_time_off (
    id            uuid PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    starts_on     date NOT NULL,
    -- Inclusive.
    ends_on       date NOT NULL,
    kind          text NOT NULL CHECK (kind IN ('vacation', 'sick', 'training', 'other')),
    note          text NOT NULL DEFAULT '' CHECK (length(note) <= 2000),
    status        text NOT NULL DEFAULT 'requested'
                  CHECK (status IN ('requested', 'approved', 'declined', 'cancelled')),
    decided_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    decided_at    timestamptz,
    decision_note text NOT NULL DEFAULT '' CHECK (length(decision_note) <= 2000),
    created_at    timestamptz NOT NULL DEFAULT now(),
    CHECK (ends_on >= starts_on),
    CHECK (ends_on - starts_on <= 366)
);
CREATE INDEX wf_time_off_user ON wf_time_off (user_id, starts_on);
CREATE INDEX wf_time_off_pending ON wf_time_off (created_at) WHERE status = 'requested';
