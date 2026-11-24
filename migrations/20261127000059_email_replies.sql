-- Public notes emailed to the requester: the message sent (its id threads
-- the requester's answer back to the ticket), to whom, and when or why not.
ALTER TABLE sd_ticket_notes
    ADD COLUMN message_id text,
    ADD COLUMN emailed_to text,
    ADD COLUMN emailed_at timestamptz,
    ADD COLUMN email_error text;
CREATE INDEX sd_ticket_notes_message ON sd_ticket_notes (message_id) WHERE message_id IS NOT NULL;
