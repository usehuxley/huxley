-- Forms (design doc §6.8): a JSON Schema (plus UI hints) people fill in;
-- each submission starts the runs of workflows with a form trigger for it.

CREATE TABLE forms (
    id           uuid PRIMARY KEY,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    key          text NOT NULL CHECK (key ~ '^[a-z][a-z0-9-]{0,63}$'),
    name         text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    description  text CHECK (length(description) <= 2000),
    -- JSON Schema of the answers (an object), and per-field UI hints.
    schema       jsonb NOT NULL,
    ui           jsonb NOT NULL DEFAULT '{}',
    archived     boolean NOT NULL DEFAULT false,
    created_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (org_id, key)
);

CREATE TABLE form_submissions (
    id            uuid PRIMARY KEY,
    form_id       uuid NOT NULL REFERENCES forms (id) ON DELETE CASCADE,
    -- The organization it was submitted for: the form's or one below it.
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    submitted_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    data          jsonb NOT NULL,
    run_ids       uuid[] NOT NULL DEFAULT '{}',
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX form_submissions_form ON form_submissions (form_id, created_at DESC);

ALTER TABLE forms ENABLE ROW LEVEL SECURITY;
CREATE POLICY forms_all ON forms FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
ALTER TABLE form_submissions ENABLE ROW LEVEL SECURITY;
CREATE POLICY form_submissions_all ON form_submissions FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));

-- Technicians fill forms in; owners and admins hold `*`.
UPDATE roles SET permissions = permissions || '{form:submit}'
 WHERE key = 'technician' AND is_system;
