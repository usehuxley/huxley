-- Change management (design doc §8.6, ADR-054): a change ticket's type,
-- risk (a questionnaire scored low to high), plans, window and affected
-- assets, through submit → decide (the Change approvals pack: CAB, then
-- client approvers for high risk) → implement → review. Freeze windows
-- block normal changes.

CREATE TABLE sd_changes (
    ticket_id        uuid PRIMARY KEY REFERENCES sd_tickets (id) ON DELETE CASCADE,
    org_id           uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    change_type      text NOT NULL DEFAULT 'normal' CHECK (change_type IN ('standard', 'normal', 'emergency')),
    -- Question key → chosen option key.
    risk_answers     jsonb NOT NULL DEFAULT '{}',
    risk_score       smallint,
    risk_level       text CHECK (risk_level IN ('low', 'medium', 'high')),
    implementation_plan text NOT NULL DEFAULT '' CHECK (length(implementation_plan) <= 20000),
    backout_plan     text NOT NULL DEFAULT '' CHECK (length(backout_plan) <= 20000),
    test_plan        text NOT NULL DEFAULT '' CHECK (length(test_plan) <= 20000),
    planned_start    timestamptz,
    planned_end      timestamptz CHECK (planned_end IS NULL OR planned_start IS NULL OR planned_end > planned_start),
    affected_assets  uuid[] NOT NULL DEFAULT '{}',
    state            text NOT NULL DEFAULT 'draft'
                     CHECK (state IN ('draft', 'submitted', 'approved', 'rejected', 'implemented', 'reviewed', 'cancelled')),
    submitted_at     timestamptz,
    decided_at       timestamptz,
    decided_by       text,
    decision_comment text,
    implemented_at   timestamptz,
    -- successful, failed or backed_out.
    outcome          text CHECK (outcome IS NULL OR outcome IN ('successful', 'failed', 'backed_out')),
    review           text CHECK (review IS NULL OR length(review) <= 20000),
    reviewed_at      timestamptz,
    version          integer NOT NULL DEFAULT 1
);
CREATE INDEX sd_changes_window ON sd_changes (planned_start, planned_end) WHERE state IN ('submitted', 'approved');
ALTER TABLE sd_changes ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_changes_all ON sd_changes FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));

-- No normal changes in these windows (an organization's and those below).
CREATE TABLE sd_change_freezes (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    starts_at   timestamptz NOT NULL,
    ends_at     timestamptz NOT NULL CHECK (ends_at > starts_at),
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sd_change_freezes_window ON sd_change_freezes (starts_at, ends_at);
ALTER TABLE sd_change_freezes ENABLE ROW LEVEL SECURITY;
CREATE POLICY sd_change_freezes_all ON sd_change_freezes FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
