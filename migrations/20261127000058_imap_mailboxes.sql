-- Fetching mail over IMAP: where from, and the password (sealed with the
-- organization's data key, bound to the mailbox).
ALTER TABLE sd_mailboxes
    -- {host, port, security: tls | start_tls | none, username, folder}
    ADD COLUMN imap jsonb,
    ADD COLUMN secret bytea,
    ADD CONSTRAINT sd_mailboxes_imap CHECK (kind <> 'imap' OR imap IS NOT NULL);
