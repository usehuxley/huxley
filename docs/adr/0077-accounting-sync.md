# ADR-077: Accounting sync: Huxley prepares and tracks, a pack's system workflow creates

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

§8.11 and Phase 11 call for accounting sync packs, QuickBooks Online and Xero first, built as integrations, with the sync as a system workflow so other platforms can be added. That includes:
- a mapping UI (companies ↔ customers, products and work types ↔ items, tax codes, classes and tracking categories);
- per-invoice sync status, an error queue and retry;
- credit memos for corrections.

Huxley stays out of payments and posting. The accounting system is the record of invoices once they're synced.

## Decision

- **Settings per provider** name the target: a sync pack's key, such as `quickbooks` or `xero`. They also set whether each invoice syncs as soon as it's approved, or only by hand.
- **Mappings** are kept in Huxley, per provider and target:
  - clients ↔ customers (the nearest client up the tree counts);
  - products, line kinds and a default ↔ items;
  - tax codes and a default ↔ tax codes or rates;
  - clients and a default ↔ classes or tracking categories.

  Work types aren't mapped yet. Time lines map by kind until invoice lines carry their work type.
- **The accounting system's lists** come from the pack's datasets (`<target>-customers`, `-items`, `-tax-codes`, `-classes`, each with `id` and `name`). A pack workflow fetches them. Huxley never calls the accounting API itself. When there are no lists, the UI takes IDs typed in.
- **Requesting a sync** prepares the invoice with every mapping resolved, and raises `invoice.sync_requested` with the prepared document:
  - customer, lines with their item, tax code and class, dates, currency and total;
  - credit memos carry positive amounts, and the document kind says it's a credit.

  A missing customer or item fails the sync at once, with what to match, so the pack never sees an incomplete invoice.
- **The pack's workflow** listens for `invoice.sync_requested`, filtered to its target. It shapes the document for its API, creates it there, and reports with `core.invoice.sync_result`:
  - synced: the remote ID, number and https link;
  - failed: the reason, taken from `on_error`.

  Synced invoices that were approved are marked sent.
- **The error queue** is the failed syncs. A retry asks again: attempts are counted, and the payload is prepared afresh, so fixed mappings take effect.

## Consequences

- Adding an accounting system means writing a pack: an integration, a workflow on `invoice.sync_requested`, and the four list datasets. Huxley needs no changes.
- Payment status back-sync (B4) can follow the same split. A pack workflow reads balances, and a `core.invoice.*` step records them.
- Within one transaction an invoice can't be both approved and synced, so the sync is asynchronous by design. The invoice page shows "Syncing" until the workflow reports.
