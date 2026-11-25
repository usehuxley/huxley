-- Sending a mailbox's replies through its own SMTP server (ADR-038), with
-- the mailbox's stored login: {host, port, security, username}.
ALTER TABLE sd_mailboxes ADD COLUMN smtp jsonb;
