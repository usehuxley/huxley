-- Sales activities (design doc §8.10, ADR-070): tasks, calls, meetings,
-- notes and emails on leads, opportunities, quotes and contacts. Emails
-- reach them through a mailbox set to log sales email (a BCC address, or a
-- synced mailbox): the Sales email logging pack files each message on the
-- contacts and leads it's to or from, instead of opening a ticket.

CREATE TABLE sales_activities (
    id               uuid PRIMARY KEY,
    org_id           uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    kind             text NOT NULL CHECK (kind IN ('task', 'call', 'meeting', 'note', 'email')),
    subject          text NOT NULL CHECK (length(btrim(subject)) BETWEEN 1 AND 300),
    body             text NOT NULL DEFAULT '' CHECK (length(body) <= 20000),
    lead_id          uuid REFERENCES sales_leads (id) ON DELETE CASCADE,
    opportunity_id   uuid REFERENCES sales_opportunities (id) ON DELETE CASCADE,
    quote_id         uuid REFERENCES sales_quotes (id) ON DELETE SET NULL,
    contact_id       uuid REFERENCES crm_contacts (id) ON DELETE SET NULL,
    -- Who it's for (tasks), or who did it.
    owner_id         uuid REFERENCES users (id) ON DELETE SET NULL,
    due_at           timestamptz,
    starts_at        timestamptz,
    ends_at          timestamptz,
    duration_minutes integer CHECK (duration_minutes BETWEEN 0 AND 1440),
    -- Calls: connected, voicemail, no answer…; meetings: held, cancelled…
    outcome          text CHECK (length(outcome) <= 100),
    location         text CHECK (length(location) <= 300),
    -- Emails: in (from them) or out (to them), and the message logged.
    direction        text CHECK (direction IN ('in', 'out')),
    email_id         uuid REFERENCES sd_inbound_emails (id) ON DELETE SET NULL,
    done_at          timestamptz,
    created_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    CHECK (ends_at IS NULL OR starts_at IS NULL OR ends_at >= starts_at)
);
CREATE INDEX sales_activities_lead ON sales_activities (lead_id, created_at DESC) WHERE lead_id IS NOT NULL;
CREATE INDEX sales_activities_opportunity ON sales_activities (opportunity_id, created_at DESC)
    WHERE opportunity_id IS NOT NULL;
CREATE INDEX sales_activities_contact ON sales_activities (contact_id, created_at DESC) WHERE contact_id IS NOT NULL;
CREATE INDEX sales_activities_owner_open ON sales_activities (owner_id, due_at) WHERE done_at IS NULL;
-- An email is logged once per contact or lead.
CREATE UNIQUE INDEX sales_activities_email_once
    ON sales_activities (email_id, coalesce(contact_id, lead_id)) WHERE email_id IS NOT NULL;

ALTER TABLE sales_activities ENABLE ROW LEVEL SECURITY;
CREATE POLICY sales_activities_all ON sales_activities FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- What a mailbox is for: tickets, or logging sales email.
ALTER TABLE sd_mailboxes
    ADD COLUMN purpose text NOT NULL DEFAULT 'tickets' CHECK (purpose IN ('tickets', 'sales_log'));
