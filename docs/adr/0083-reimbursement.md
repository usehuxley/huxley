# ADR-083: Expenses are paid back as a bill to the person through the accounting packs

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

§8.16: reimbursement is marked paid, exported, or synced to the accounting system as bills or expense claims through the accounting packs. Phase 12's exit criteria sync the reimbursable items to the accounting sandbox as an expense claim. Neither QuickBooks Online nor Xero offers a dependable API for expense claims today. Xero's claims API is deprecated, and QuickBooks has none. Both pay people back through bills to the person as a supplier.

## Decision

- **Three ways to pay back an approved report:**
  - send it to accounting;
  - mark it paid back;
  - export it as CSV, with formula-looking cells defused.
- **Sending** works like the invoice sync (ADR-077). Huxley prepares the bill with its mappings and raises `expense_report.sync_requested`. The pack's workflow, filtered to its target, creates it and reports back with `core.expense.sync_result`.
  - The bill goes to the person's **supplier** (a new mapping kind: user ↔ supplier).
  - It has one line per reimbursable expense, at the amount with tax included, on the category's **expense account** (a new mapping kind: category or default ↔ account).
  - It's dated the day the report was approved, and numbered with the report's reference.
  - A report in more than one currency can't be sent; an unmatched person or category fails it, saying what to map.
- **The packs:**
  - QuickBooks creates a Bill to the vendor, with account-based expense lines.
  - Xero creates an authorised ACCPAY invoice (a bill) to the contact, amounts tax inclusive, by account code.
  - Fetch lists adds suppliers (QuickBooks vendors, Xero contacts) and expense accounts.
- **Paid back** is a separate step, by hand: the report and its entries become `reimbursed`. A synced bill isn't paid until someone pays it in the accounting system.

## Consequences

- If either accounting system restores an expense claims API, a pack can use it from the same event without changes in Huxley.
- Status back-sync for bills (paid in the accounting system, so marked paid back) can follow the invoice status model (ADR-078).
