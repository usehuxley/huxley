-- Public links to forms (design doc §6.8): anyone with the link can fill
-- the form in for one organization. Only a hash of the token is kept.
CREATE TABLE form_links (
    id            uuid PRIMARY KEY,
    form_id       uuid NOT NULL REFERENCES forms (id) ON DELETE CASCADE,
    -- The organization submissions are for: the form's or one below it.
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    token_hash    bytea NOT NULL UNIQUE,
    name          text NOT NULL,
    expires_at    timestamptz,
    revoked_at    timestamptz,
    last_used_at  timestamptz,
    created_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX form_links_form ON form_links (form_id);

ALTER TABLE form_links ENABLE ROW LEVEL SECURITY;
CREATE POLICY form_links_all ON form_links FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));

-- Which link a submission came through (no user: it's anonymous).
ALTER TABLE form_submissions ADD COLUMN link_id uuid REFERENCES form_links (id) ON DELETE SET NULL;
