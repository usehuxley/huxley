-- Internal invoice approval (design doc §8.11, ADR-076): no invoice goes to
-- a client, the accounting system or any other integration until it's
-- approved. Rules decide which invoices need a review and who reviews them
-- (a hand-edited invoice does, by default); the review is a workflow (a
-- built-in pack on the approval step); comments on the invoice or a line
-- mention people; an override is recorded. Billing runs are approved in one
-- pass, with the exceptions flagged.

CREATE TABLE billing_review_rules (
    id           uuid PRIMARY KEY,
    name         text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    kind         text NOT NULL CHECK (kind IN ('always', 'edited', 'total_above', 'change_above', 'new_client',
                                               'credit_memo', 'expression')),
    threshold    numeric(18,4),
    expression   text CHECK (length(expression) <= 2000),
    -- Approval step approvers: role:<key>, team:<id>, user:<id>.
    reviewers    text NOT NULL DEFAULT 'role:admin' CHECK (length(reviewers) BETWEEN 1 AND 500),
    position     integer NOT NULL DEFAULT 0,
    active       boolean NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CHECK (kind NOT IN ('total_above', 'change_above') OR threshold IS NOT NULL),
    CHECK (kind <> 'expression' OR expression IS NOT NULL)
);

-- An invoice changed by hand needs approval, out of the box.
INSERT INTO billing_review_rules (id, name, kind, reviewers, position)
VALUES ('01970000-0000-7000-8000-00000000b111', 'Changed by hand', 'edited', 'role:admin', 1);

ALTER TABLE billing_invoices
    -- Bumped when its lines or notes change (holds don't count).
    ADD COLUMN revision integer NOT NULL DEFAULT 1,
    ADD COLUMN review_status text NOT NULL DEFAULT 'not_needed'
        CHECK (review_status IN ('not_needed', 'needed', 'in_review', 'approved', 'changes_requested', 'rejected')),
    ADD COLUMN review_revision integer,
    -- Rules that match it now: [{ruleId, name, detail, reviewers}].
    ADD COLUMN review_reasons jsonb NOT NULL DEFAULT '[]',
    -- Approved without an approved review, by someone allowed to.
    ADD COLUMN review_overridden boolean NOT NULL DEFAULT false;

CREATE TABLE billing_invoice_reviews (
    id              uuid PRIMARY KEY,
    invoice_id      uuid NOT NULL REFERENCES billing_invoices (id) ON DELETE CASCADE,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    revision        integer NOT NULL,
    snapshot        jsonb NOT NULL,
    reasons         jsonb NOT NULL DEFAULT '[]',
    reviewers       text[] NOT NULL DEFAULT '{}',
    submitted_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    submitted_at    timestamptz NOT NULL DEFAULT now(),
    decision        text CHECK (decision IN ('approved', 'changes_requested', 'rejected')),
    decided_by      text CHECK (length(decided_by) <= 200),
    decided_at      timestamptz,
    comment         text CHECK (length(comment) <= 2000)
);
CREATE INDEX billing_invoice_reviews_invoice ON billing_invoice_reviews (invoice_id, submitted_at);

CREATE TABLE billing_invoice_comments (
    id           uuid PRIMARY KEY,
    invoice_id   uuid NOT NULL REFERENCES billing_invoices (id) ON DELETE CASCADE,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    line_id      uuid,
    line_name    text CHECK (length(line_name) <= 1000),
    parent_id    uuid REFERENCES billing_invoice_comments (id) ON DELETE CASCADE,
    author_id    uuid REFERENCES users (id) ON DELETE SET NULL,
    body         text NOT NULL CHECK (length(btrim(body)) BETWEEN 1 AND 4000),
    mentions     uuid[] NOT NULL DEFAULT '{}',
    resolved_at  timestamptz,
    resolved_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX billing_invoice_comments_invoice ON billing_invoice_comments (invoice_id, created_at);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['billing_invoice_reviews', 'billing_invoice_comments'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;
