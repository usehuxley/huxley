-- The portal's request catalog (ADR-040): forms offered to client contacts
-- of the form's organization and those below it. Submitting runs the
-- form's workflows, as with a public link.

ALTER TABLE forms
    ADD COLUMN portal boolean NOT NULL DEFAULT false,
    -- Groups forms in the portal, e.g. "Accounts" or "Hardware".
    ADD COLUMN portal_category text
        CHECK (portal_category IS NULL OR length(btrim(portal_category)) BETWEEN 1 AND 60);

ALTER TABLE form_submissions
    ADD COLUMN portal_account_id uuid REFERENCES portal_accounts (id) ON DELETE SET NULL;
CREATE INDEX form_submissions_portal_account ON form_submissions (portal_account_id, created_at DESC)
    WHERE portal_account_id IS NOT NULL;
