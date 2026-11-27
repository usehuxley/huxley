-- Internal quote review (design doc §8.10, ADR-066): rules decide when a
-- quote needs review and who reviews it; submitting one raises
-- quote.submitted_for_review for the review workflow (a built-in pack on
-- the approval step); reviewers approve, request changes or reject; a
-- quote that needs review can't be sent until approved, unless someone
-- with the override permission sends it anyway. Comments, on the quote or
-- a line, mention people and can be resolved.

CREATE TABLE sales_review_rules (
    id           uuid PRIMARY KEY,
    name         text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    kind         text NOT NULL CHECK (kind IN ('always', 'margin_below', 'first_year_above',
                                               'discount_above', 'new_client', 'category', 'expression')),
    threshold    numeric(18,4),
    categories   text[] NOT NULL DEFAULT '{}',
    expression   text CHECK (length(expression) <= 2000),
    -- Approval step approvers: role:<key>, team:<id>, user:<id>.
    reviewers    text NOT NULL DEFAULT 'role:admin' CHECK (length(reviewers) BETWEEN 1 AND 500),
    position     integer NOT NULL DEFAULT 0,
    active       boolean NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CHECK (kind NOT IN ('margin_below', 'first_year_above', 'discount_above') OR threshold IS NOT NULL),
    CHECK (kind <> 'expression' OR expression IS NOT NULL)
);

ALTER TABLE sales_quotes
    ADD COLUMN review_status text NOT NULL DEFAULT 'not_needed'
        CHECK (review_status IN ('not_needed', 'needed', 'in_review', 'approved', 'changes_requested', 'rejected')),
    -- The revision the latest review was of.
    ADD COLUMN review_revision integer,
    -- Rules that match it now: [{ruleId, name, detail, reviewers}].
    ADD COLUMN review_reasons jsonb NOT NULL DEFAULT '[]',
    -- Sent without an approved review, by someone allowed to.
    ADD COLUMN sent_overridden boolean NOT NULL DEFAULT false;

-- Each submission for review: what was reviewed and the decision.
CREATE TABLE sales_quote_reviews (
    id              uuid PRIMARY KEY,
    quote_id        uuid NOT NULL REFERENCES sales_quotes (id) ON DELETE CASCADE,
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
CREATE INDEX sales_quote_reviews_quote ON sales_quote_reviews (quote_id, submitted_at);

CREATE TABLE sales_quote_comments (
    id           uuid PRIMARY KEY,
    quote_id     uuid NOT NULL REFERENCES sales_quotes (id) ON DELETE CASCADE,
    org_id       uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- On a line (kept as its name too, should the line go), or the quote.
    line_id      uuid,
    line_name    text CHECK (length(line_name) <= 300),
    -- Replies hang off the thread's first comment.
    parent_id    uuid REFERENCES sales_quote_comments (id) ON DELETE CASCADE,
    author_id    uuid REFERENCES users (id) ON DELETE SET NULL,
    body         text NOT NULL CHECK (length(btrim(body)) BETWEEN 1 AND 4000),
    mentions     uuid[] NOT NULL DEFAULT '{}',
    resolved_at  timestamptz,
    resolved_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sales_quote_comments_quote ON sales_quote_comments (quote_id, created_at);

ALTER TABLE sales_quote_reviews ENABLE ROW LEVEL SECURITY;
ALTER TABLE sales_quote_comments ENABLE ROW LEVEL SECURITY;
DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['sales_quote_reviews', 'sales_quote_comments'] LOOP
        EXECUTE format(
            'CREATE POLICY %I ON %I FOR ALL TO huxley_app
               USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
               WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))',
            t || '_all', t);
    END LOOP;
END $$;
