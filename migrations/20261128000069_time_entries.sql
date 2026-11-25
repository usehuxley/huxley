-- Time entries (design doc §8.2, §8.15): time logged on tickets, or as
-- non-ticket time with a work type. Timesheets (Phase 6) build on these.

-- What kind of work: instance-wide, seeded, and editable.
CREATE TABLE sd_work_types (
    id          uuid PRIMARY KEY,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 60),
    -- The default for entries of this type.
    billable    boolean NOT NULL DEFAULT true,
    -- Offered on tickets (else for non-ticket time).
    on_tickets  boolean NOT NULL DEFAULT true,
    position    integer NOT NULL DEFAULT 0,
    version     integer NOT NULL DEFAULT 1,
    created_at  timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz
);
CREATE UNIQUE INDEX sd_work_types_name ON sd_work_types (lower(name)) WHERE archived_at IS NULL;
INSERT INTO sd_work_types (id, name, billable, on_tickets, position) VALUES
    (gen_random_uuid(), 'Remote support', true, true, 1),
    (gen_random_uuid(), 'Onsite', true, true, 2),
    (gen_random_uuid(), 'Project work', true, true, 3),
    (gen_random_uuid(), 'Admin', false, false, 4),
    (gen_random_uuid(), 'Internal meeting', false, false, 5),
    (gen_random_uuid(), 'Training', false, false, 6),
    (gen_random_uuid(), 'Travel', false, true, 7);

CREATE TABLE sd_time_entries (
    id           uuid PRIMARY KEY,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- The ticket's organization; none for non-ticket time (system scope
    -- only, by its owner).
    org_id       uuid REFERENCES organizations (id) ON DELETE CASCADE,
    ticket_id    uuid REFERENCES sd_tickets (id) ON DELETE CASCADE,
    work_type_id uuid NOT NULL REFERENCES sd_work_types (id),
    -- The day the work was done (the person's own calendar day).
    worked_on    date NOT NULL,
    minutes      integer NOT NULL CHECK (minutes BETWEEN 1 AND 1440),
    billable     boolean NOT NULL,
    notes        text NOT NULL DEFAULT '' CHECK (length(notes) <= 4000),
    version      integer NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CHECK ((ticket_id IS NULL) = (org_id IS NULL))
);
CREATE INDEX sd_time_entries_ticket ON sd_time_entries (ticket_id) WHERE ticket_id IS NOT NULL;
CREATE INDEX sd_time_entries_user ON sd_time_entries (user_id, worked_on);
ALTER TABLE sd_time_entries ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_time_entries_all ON sd_time_entries FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
