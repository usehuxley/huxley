-- Client contacts deciding workflow approvals in the portal (ADR-040).
-- An approval step can name a contact (`contact:<id>`) or the run's
-- company's client approvers (`client_approvers`); their portal accounts
-- are kept here, alongside the staff approvers.

ALTER TABLE approvals
    ADD COLUMN portal_approvers uuid[] NOT NULL DEFAULT '{}',
    ADD COLUMN decided_by_portal uuid REFERENCES portal_accounts (id) ON DELETE SET NULL;

CREATE INDEX approvals_portal_approvers ON approvals USING gin (portal_approvers) WHERE status = 'pending';
