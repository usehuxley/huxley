-- Expense reports (design doc §8.16, ADR-081): a person bundles their
-- entries and submits them to their team lead (or administrators) through
-- an approval workflow (the built-in Expense approval pack). Approved
-- entries are locked; entries sent back can be changed and submitted
-- again. Comments, on the report or an entry, mention people.

CREATE TABLE exp_reports (
    id            uuid PRIMARY KEY,
    -- The MSP the entries' clients are under.
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id       uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    number        bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    title         text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
    status        text NOT NULL DEFAULT 'submitted'
                    CHECK (status IN ('submitted', 'approved', 'changes_requested', 'rejected', 'reimbursed')),
    reviewers     text[] NOT NULL DEFAULT '{}',
    submitted_at  timestamptz NOT NULL DEFAULT now(),
    decided_by    text CHECK (length(decided_by) <= 200),
    decided_at    timestamptz,
    comment       text CHECK (length(comment) <= 2000),
    version       integer NOT NULL DEFAULT 1
);
CREATE INDEX exp_reports_user ON exp_reports (user_id, submitted_at DESC);

ALTER TABLE exp_entries ADD COLUMN report_id uuid REFERENCES exp_reports (id) ON DELETE SET NULL;
CREATE INDEX exp_entries_report ON exp_entries (report_id) WHERE report_id IS NOT NULL;

CREATE TABLE exp_report_comments (
    id          uuid PRIMARY KEY,
    report_id   uuid NOT NULL REFERENCES exp_reports (id) ON DELETE CASCADE,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    entry_id    uuid REFERENCES exp_entries (id) ON DELETE SET NULL,
    author_id   uuid REFERENCES users (id) ON DELETE SET NULL,
    body        text NOT NULL CHECK (length(btrim(body)) BETWEEN 1 AND 4000),
    mentions    uuid[] NOT NULL DEFAULT '{}',
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX exp_report_comments_report ON exp_report_comments (report_id, created_at);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['exp_reports', 'exp_report_comments'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;
