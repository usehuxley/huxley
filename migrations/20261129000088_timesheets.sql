-- Timesheets (design doc §8.15): a person's time for a period, submitted,
-- then approved (locking its entries) or rejected with comments, by their
-- team lead or someone who manages the workforce.

-- Team leads review their members' timesheets.
ALTER TABLE team_members ADD COLUMN lead boolean NOT NULL DEFAULT false;

-- One row: how periods are cut. Weekly from Monday by default.
CREATE TABLE wf_timesheet_settings (
    id           integer PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    period_days  integer NOT NULL DEFAULT 7 CHECK (period_days IN (7, 14)),
    -- A day periods start on (any period's first day).
    anchor       date NOT NULL DEFAULT '2024-01-01',
    -- Billing previews use only time on approved timesheets.
    bill_approved_only boolean NOT NULL DEFAULT false,
    updated_at   timestamptz NOT NULL DEFAULT now()
);
INSERT INTO wf_timesheet_settings (id) VALUES (1);

CREATE TABLE wf_timesheets (
    id             uuid PRIMARY KEY,
    user_id        uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    period_start   date NOT NULL,
    period_end     date NOT NULL CHECK (period_end >= period_start),
    status         text NOT NULL DEFAULT 'open'
                   CHECK (status IN ('open', 'submitted', 'approved', 'rejected')),
    submitted_at   timestamptz,
    decided_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    decided_at     timestamptz,
    version        integer NOT NULL DEFAULT 1,
    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, period_start)
);
CREATE INDEX wf_timesheets_status ON wf_timesheets (status, period_start);
CREATE INDEX wf_timesheets_user_period ON wf_timesheets (user_id, period_start, period_end);

-- What happened to each timesheet, with comments.
CREATE TABLE wf_timesheet_events (
    id            uuid PRIMARY KEY,
    timesheet_id  uuid NOT NULL REFERENCES wf_timesheets (id) ON DELETE CASCADE,
    action        text NOT NULL CHECK (action IN ('submitted', 'approved', 'rejected', 'reopened', 'withdrawn')),
    by_user       uuid REFERENCES users (id) ON DELETE SET NULL,
    note          text NOT NULL DEFAULT '' CHECK (length(note) <= 2000),
    at            timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX wf_timesheet_events_sheet ON wf_timesheet_events (timesheet_id, at);

-- Entries on a submitted or approved timesheet's days can't change (the API
-- checks first, with a clearer message; this is the backstop).
CREATE FUNCTION huxley_time_entry_unlocked() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    r record;
BEGIN
    FOR r IN
        SELECT * FROM (VALUES (OLD.user_id, OLD.worked_on), (NEW.user_id, NEW.worked_on)) AS v (u, d)
         WHERE u IS NOT NULL
    LOOP
        IF EXISTS (SELECT 1 FROM wf_timesheets
                    WHERE user_id = r.u AND r.d BETWEEN period_start AND period_end
                      AND status IN ('submitted', 'approved')) THEN
            RAISE EXCEPTION 'timesheet_locked' USING ERRCODE = 'P0001',
                HINT = 'That period''s timesheet is submitted or approved.';
        END IF;
    END LOOP;
    RETURN COALESCE(NEW, OLD);
END $$;

CREATE TRIGGER sd_time_entries_unlocked
    BEFORE INSERT OR UPDATE OR DELETE ON sd_time_entries
    FOR EACH ROW EXECUTE FUNCTION huxley_time_entry_unlocked();

-- Everyone submits their own; technicians included.
UPDATE roles SET permissions = permissions || '{timesheet:submit}'
 WHERE key IN ('technician') AND is_system;
