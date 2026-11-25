-- Appointments (design doc §8.2, the dispatch calendar): scheduled work
-- on a ticket, for a person: an onsite visit, a remote session.
CREATE TABLE sd_appointments (
    id         uuid PRIMARY KEY,
    ticket_id  uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    -- The ticket's organization.
    org_id     uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    starts_at  timestamptz NOT NULL,
    ends_at    timestamptz NOT NULL,
    notes      text NOT NULL DEFAULT '' CHECK (length(notes) <= 2000),
    created_by uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    version    integer NOT NULL DEFAULT 1,
    CHECK (ends_at > starts_at),
    CHECK (ends_at - starts_at <= interval '24 hours')
);
CREATE INDEX sd_appointments_user ON sd_appointments (user_id, starts_at);
CREATE INDEX sd_appointments_ticket ON sd_appointments (ticket_id);
ALTER TABLE sd_appointments ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_appointments_all ON sd_appointments FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
