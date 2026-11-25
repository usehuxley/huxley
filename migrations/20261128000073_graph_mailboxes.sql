-- Fetching mail through Microsoft Graph (ADR-038): which Graph connection,
-- whose mailbox and which folder.
ALTER TABLE sd_mailboxes
    -- {connection, user, folder}
    ADD COLUMN graph jsonb,
    ADD CONSTRAINT sd_mailboxes_graph CHECK (kind <> 'graph' OR graph IS NOT NULL) NOT VALID;
