-- wait_for_callback (design doc §6.4): a single-use URL that resumes a
-- parked step. Only a hash of its token is stored.
CREATE TABLE callbacks (
    token_hash  bytea PRIMARY KEY,
    run_id      uuid NOT NULL,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    step_key    text NOT NULL,
    attempt     integer NOT NULL,
    expires_at  timestamptz NOT NULL,
    -- When the step told the other system (its `send`) about the URL.
    sent_at     timestamptz,
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX callbacks_run ON callbacks (run_id);

ALTER TABLE callbacks ENABLE ROW LEVEL SECURITY;
CREATE POLICY callbacks_all ON callbacks FOR ALL TO huxley_app
    USING (huxley_org_in_scope(org_id)) WITH CHECK (huxley_org_in_scope(org_id));
