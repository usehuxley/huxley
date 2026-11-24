-- Email intake (ADR-038): mailboxes and the messages they receive.

CREATE TABLE sd_mailboxes (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name            text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    address         text NOT NULL CHECK (address = lower(address) AND address LIKE '%_@_%'),
    -- How mail arrives: posted to it (`forward`), or fetched.
    kind            text NOT NULL DEFAULT 'forward' CHECK (kind IN ('forward', 'imap', 'graph')),
    -- New tickets from it.
    ticket_type     text NOT NULL DEFAULT 'incident',
    queue_id        uuid REFERENCES sd_queues (id) ON DELETE SET NULL,
    -- A sender who isn't a contact: make one, open the ticket without a
    -- requester, or ignore the message.
    unknown_sender  text NOT NULL DEFAULT 'create_contact'
                    CHECK (unknown_sender IN ('create_contact', 'no_requester', 'ignore')),
    -- Where unknown senders' tickets (and contacts) go; none: the mailbox's.
    unknown_org_id  uuid REFERENCES organizations (id) ON DELETE SET NULL,
    strip_replies   boolean NOT NULL DEFAULT true,
    enabled         boolean NOT NULL DEFAULT true,
    -- Fetching state (imap, graph).
    cursor          jsonb NOT NULL DEFAULT '{}'::jsonb,
    last_checked_at timestamptz,
    last_error      text,
    version         integer NOT NULL DEFAULT 1,
    created_at      timestamptz NOT NULL DEFAULT now(),
    archived_at     timestamptz
);
CREATE UNIQUE INDEX sd_mailboxes_address ON sd_mailboxes (address) WHERE archived_at IS NULL;

CREATE TABLE sd_inbound_emails (
    id             uuid PRIMARY KEY,
    org_id         uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    mailbox_id     uuid NOT NULL REFERENCES sd_mailboxes (id) ON DELETE CASCADE,
    message_id     text NOT NULL,
    in_reply_to    text[] NOT NULL DEFAULT '{}',
    refs           text[] NOT NULL DEFAULT '{}',
    from_address   text,
    from_name      text,
    to_addresses   jsonb NOT NULL DEFAULT '[]'::jsonb,
    cc_addresses   jsonb NOT NULL DEFAULT '[]'::jsonb,
    subject        text NOT NULL DEFAULT '',
    body_text      text NOT NULL DEFAULT '',
    -- Why it looks automatic, if it does.
    automatic      text,
    -- The raw message in object storage.
    raw_key        text NOT NULL,
    size_bytes     bigint NOT NULL,
    -- [{filename, contentType, size, inline}]
    attachments    jsonb NOT NULL DEFAULT '[]'::jsonb,
    received_at    timestamptz NOT NULL DEFAULT now(),
    -- What became of it: ticket_created, note_added, ticket_reopened,
    -- ignored_automatic, ignored_loop, ignored_unknown_sender.
    outcome        text,
    outcome_detail text,
    processed_at   timestamptz,
    ticket_id      uuid REFERENCES sd_tickets (id) ON DELETE SET NULL,
    UNIQUE (mailbox_id, message_id)
);
CREATE INDEX sd_inbound_emails_message ON sd_inbound_emails (message_id);
CREATE INDEX sd_inbound_emails_sender ON sd_inbound_emails (mailbox_id, from_address, received_at);
CREATE INDEX sd_inbound_emails_recent ON sd_inbound_emails (mailbox_id, received_at DESC);

-- A note that came by email, and who sent it (they may not be a contact).
ALTER TABLE sd_ticket_notes
    ADD COLUMN email_id uuid REFERENCES sd_inbound_emails (id) ON DELETE SET NULL,
    ADD COLUMN author_email text;

ALTER TABLE sd_mailboxes ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_mailboxes_all ON sd_mailboxes FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
ALTER TABLE sd_inbound_emails ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_inbound_emails_all ON sd_inbound_emails FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
