-- Customer satisfaction surveys (design doc §8.9, ADR-043): sent when a
-- ticket is resolved, answered by one-click email link or in the portal.

CREATE TABLE cx_surveys (
    id            uuid PRIMARY KEY,
    -- The ticket's company.
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    ticket_id     uuid NOT NULL REFERENCES sd_tickets (id) ON DELETE CASCADE,
    contact_id    uuid REFERENCES crm_contacts (id) ON DELETE SET NULL,
    -- Who the ticket was assigned to when it was resolved.
    assignee_user_id uuid REFERENCES users (id) ON DELETE SET NULL,
    -- 'five' (1 to 5) or 'thumbs' (1 down, 5 up).
    scale         text NOT NULL CHECK (scale IN ('five', 'thumbs')),
    -- The answer link's token (hashed); single answer, until it expires.
    token_hash    bytea NOT NULL UNIQUE,
    emailed       boolean NOT NULL DEFAULT false,
    rating        smallint CHECK (rating BETWEEN 1 AND 5),
    comment       text CHECK (length(comment) <= 4000),
    -- How it was answered: 'email' (the link) or 'portal'.
    channel       text CHECK (channel IN ('email', 'portal')),
    sent_at       timestamptz NOT NULL DEFAULT now(),
    responded_at  timestamptz,
    expires_at    timestamptz NOT NULL
);
CREATE INDEX cx_surveys_ticket ON cx_surveys (ticket_id, sent_at DESC);
CREATE INDEX cx_surveys_org ON cx_surveys (org_id, sent_at DESC);
ALTER TABLE cx_surveys ENABLE ROW LEVEL SECURITY;
CREATE POLICY cx_surveys_all ON cx_surveys FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
