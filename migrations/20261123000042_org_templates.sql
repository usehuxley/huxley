-- Your organization's templates (design doc §6.6), saved from workflows:
-- shared with the organizations below it, like the script library.
CREATE TABLE workflow_templates (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name         text NOT NULL,
    description  text NOT NULL DEFAULT '',
    category     text NOT NULL,
    workflow     jsonb NOT NULL,
    form         jsonb,
    created_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX workflow_templates_org ON workflow_templates (org_id);

ALTER TABLE workflow_templates ENABLE ROW LEVEL SECURITY;
CREATE POLICY workflow_templates_all ON workflow_templates FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
