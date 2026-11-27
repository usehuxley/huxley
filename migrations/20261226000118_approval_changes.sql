-- Approvals that may ask for changes (ADR-066): an approval step with
-- `changes: true` lets approvers approve, reject or request changes.

ALTER TYPE approval_status ADD VALUE IF NOT EXISTS 'changes_requested';
ALTER TABLE approvals ADD COLUMN allow_changes boolean NOT NULL DEFAULT false;
