-- The approval step (design doc §6.2): a run parks until one of the
-- approvers decides. The approver list is fixed when the step parks.
CREATE TYPE approval_status AS ENUM ('pending', 'approved', 'rejected', 'expired');

CREATE TABLE approvals (
    id           uuid PRIMARY KEY,
    run_id       uuid NOT NULL,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    step_key     text NOT NULL,
    attempt      integer NOT NULL,
    title        text NOT NULL,
    message      text,
    approvers    uuid[] NOT NULL,
    status       approval_status NOT NULL DEFAULT 'pending',
    expires_at   timestamptz NOT NULL,
    notified_at  timestamptz,
    decided_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    decided_at   timestamptz,
    comment      text CHECK (length(comment) <= 2000),
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (run_id, step_key, attempt)
);

CREATE INDEX approvals_approvers ON approvals USING gin (approvers) WHERE status = 'pending';

ALTER TABLE approvals ENABLE ROW LEVEL SECURITY;
CREATE POLICY approvals_all ON approvals FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
