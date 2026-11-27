# ADR-079: Expenses are entries placed like time, with receipts kept beside them

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Phase 12 adds the `expenses` module (§8.16):
- entries with categories, receipts, tax, and billable and reimbursable flags;
- mileage at configurable rates;
- logged against an in-progress ticket, a project (a phase or a task), a client, or internally;
- reports and approval, billing with markups and agreement coverage, project cost burn, reimbursement through the accounting packs, and card transactions.

This first slice lays down entries, categories, mileage and receipts. The rest builds on them.

## Decision

- **Categories are instance-wide.** Each one is mileage or an amount, and sets:
  - whether entries are billable by default, and the markup;
  - whether they're reimbursable by default;
  - a limit over which an entry is flagged;
  - whether a receipt is needed.

  Six categories ship.
- **Mileage rates** are per unit (`mi` or `km`) and currency, from a day. The rate in effect on the day spent is copied onto the entry, so later rate changes don't alter it.
- **Placement** decides which organization owns the cost:
  - a ticket, which must not be closed, gives its client, and its project when the ticket is a project task;
  - a project, optionally with a phase, gives its client;
  - otherwise, a client or the provider (internal).

  The entry lives in that organization for row-level security and reporting, and belongs to the person who logged it.
- **Access:**
  - `expense:write` at the organization logs your own expenses there. You always see your own entries and can change your drafts.
  - `expense:read` sees everyone's.
  - Entries can be changed only as drafts, or after they're sent back. Reports (E3) lock them.
- **Receipts** are one file per entry, in object storage. The expenses module keeps them, not generic attachments, because receipts follow the entry's lifecycle and permissions. Only common image types and PDF are accepted (no SVG), and they're served with `nosniff`.
- **Policy flags** (over the limit, no receipt) are worked out when entries are read, so changing a category's policy shows on entries straight away.
- **Money:** the amount, tax included, plus the tax, in the entry's currency. What's billed is the amount marked up.

## Consequences

- Expense reports and approval (E3) will move entries from draft to submitted to approved or sent back to draft, and lock them once approved.
- Billing (E4) will take approved billable entries as invoice sources, marked billed once. Project cost burn will add entry amounts.
- AI receipt reading (E2) will fill in the amount, date and vendor from the receipt, for the person to confirm.
