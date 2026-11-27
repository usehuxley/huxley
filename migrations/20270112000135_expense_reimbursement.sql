-- Reimbursement (design doc §8.16, ADR-083): approved expense reports are
-- marked paid back, exported, or sent to the accounting system as a bill to
-- the person (as a supplier) through the accounting pack, with each
-- category's expense account.

ALTER TABLE billing_mappings DROP CONSTRAINT billing_mappings_kind_check;
ALTER TABLE billing_mappings ADD CONSTRAINT billing_mappings_kind_check
    CHECK (kind IN ('customer', 'item', 'tax_code', 'class', 'supplier', 'account'));

ALTER TABLE exp_reports
    ADD COLUMN sync_target     text,
    ADD COLUMN sync_status     text CHECK (sync_status IN ('pending', 'synced', 'failed')),
    ADD COLUMN sync_error      text CHECK (length(sync_error) <= 4000),
    ADD COLUMN remote_id       text CHECK (length(remote_id) <= 200),
    ADD COLUMN remote_number   text CHECK (length(remote_number) <= 100),
    ADD COLUMN remote_url      text CHECK (length(remote_url) <= 2000),
    ADD COLUMN synced_at       timestamptz,
    ADD COLUMN reimbursed_at   timestamptz,
    ADD COLUMN reimbursed_by   uuid REFERENCES users (id) ON DELETE SET NULL;
