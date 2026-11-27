# ADR-078: Payment status comes back through the sync pack; Huxley keeps client balances and credit holds

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Phase 11 asks for paid, balance and overdue status to be pulled from the accounting system onto companies. That status feeds the client portal, client health and optional credit-hold events. The portal lists invoices with their status and a link to the accounting system's view-and-pay page. Huxley still takes no payments and keeps no receivables ledger (§18.1). The accounting system stays the record of what's paid.

## Decision

- **The same split as the sync (ADR-077).** A pack workflow reads the status from the accounting system and reports it. Huxley records it and works out what follows from it.
  - `core.invoice.unpaid` lists the synced invoices of a target that aren't known to be paid, least recently checked first. `needsLink` marks the first check, when there's no pay link yet.
  - `core.invoice.status` takes statuses by the accounting system's IDs: amount due and paid, its own status word, and an https pay link. Fields left out stay as they were.
  - The QuickBooks and Xero packs run hourly, one batch query per run. They fetch each invoice's pay link once: QuickBooks' InvoiceLink, or Xero's online invoice.
- **Paid** means the amount due is zero. It records the paid date and raises `invoice.paid` once.
- **Client position.** A client's balance, overdue amount and oldest overdue due date come from its synced, sent invoices. They're kept in `billing_client_status` and worked out again whenever a status for the client is recorded. Credit memos and unsynced invoices don't count. Without a sync, Huxley has no way to know what's paid.
- **Credit hold** is optional, per provider (on the sync settings). A client goes on hold when either:
  - an invoice is at least N days overdue;
  - the overdue total passes an amount.

  Entering and leaving the hold raise `company.credit_hold` and `company.credit_hold_released`, with the reason, and are audited. What a hold does is up to workflows. Huxley doesn't block anything by itself.
- **Portal.** Client approvers and admins (`portal.invoice:read`) see the company's sent invoices and credit memos: due, overdue, paid or credit, the amount left to pay, a PDF, and **Pay** when there's a link.

## Consequences

- Client health can take the client's position as a factor. Finance dashboards (billed against unbilled, aging) can read `billing_client_status` and the syncs.
- Overdue amounts move with the calendar, but they're only worked out again when a status is recorded. The hourly status workflow re-reads every unpaid invoice, so a client crosses a threshold within the hour of its due date passing.
