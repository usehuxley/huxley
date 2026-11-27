-- Card transactions (design doc §8.16, ADR-084): company card charges,
-- imported (CSV or the API, e.g. from a bank integration) and matched to
-- expense entries, automatically (same person, amount and nearby day) or by
-- hand; one can be logged as an expense, or ignored.

CREATE TABLE exp_card_transactions (
    id            uuid PRIMARY KEY,
    -- The MSP whose card it is.
    org_id        uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- Whose card, when known.
    user_id       uuid REFERENCES users (id) ON DELETE SET NULL,
    posted_on     date NOT NULL,
    description   text NOT NULL CHECK (length(btrim(description)) BETWEEN 1 AND 500),
    merchant      text CHECK (length(merchant) <= 200),
    amount        numeric(18,4) NOT NULL,
    currency      text NOT NULL DEFAULT 'USD' CHECK (currency ~ '^[A-Z]{3}$'),
    card_last4    text CHECK (card_last4 ~ '^[0-9]{4}$'),
    -- The bank's own ID, so an import twice doesn't double up.
    external_id   text CHECK (length(external_id) <= 200),
    expense_id    uuid REFERENCES exp_entries (id) ON DELETE SET NULL,
    status        text NOT NULL DEFAULT 'unmatched' CHECK (status IN ('unmatched', 'matched', 'ignored')),
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX exp_card_transactions_external ON exp_card_transactions (org_id, external_id)
    WHERE external_id IS NOT NULL;
CREATE UNIQUE INDEX exp_card_transactions_expense ON exp_card_transactions (expense_id) WHERE expense_id IS NOT NULL;
CREATE INDEX exp_card_transactions_status ON exp_card_transactions (org_id, status, posted_on DESC);

ALTER TABLE exp_card_transactions ENABLE ROW LEVEL SECURITY;
CREATE POLICY exp_card_transactions_all ON exp_card_transactions FOR ALL TO huxley_app
    USING ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]))
    WITH CHECK ((SELECT huxley_scope_all()) OR org_id = ANY ((SELECT huxley_scope_ids())::uuid[]));
