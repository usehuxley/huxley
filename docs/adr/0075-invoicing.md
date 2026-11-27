# ADR-075: Invoices are generated per client and currency from marked sources, drafts until approved, corrected by credit memos

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Phase 11 starts invoicing (§8.11). Huxley generates invoices from what it tracks and syncs them to accounting. The sources are agreement-aware time, sales order lines, agreements' recurring charges with proration, and project billing (fixed fee, milestone, time and materials, not-to-exceed). Expenses come in Phase 12. Billing runs produce drafts for review, billed sources are marked so nothing is billed twice, and corrections after sending are credit memos.

## Decision

- **A billing crate and module**, released now. It covers settings per provider (default hourly rate, currency, payment terms, approved-time-only), runs, invoices and lines, and a `billing_billed` table that marks each source once: a time entry, sales order line, project fee or milestone, or an agreement line per period.
- **Generation** finds, for each client and currency:
  - **Time:** entries are evaluated by the existing coverage engine. Billable minutes, rounded to the agreement's increment, are billed at the agreement's rate or the provider's default. Lines are per ticket and rate.
    - Time on fixed-fee, milestone or agreement-billed projects follows those methods.
    - Not-to-exceed projects stop at the project's cap: the fee minus time already billed.
  - **Products:** one-time sales order lines delivered in full, at the agreed price after discount.
  - **Recurring:**
    - Agreement lines are billed in calendar periods (month, quarter, year).
    - A period is billed in the run whose window its start falls in. The period an agreement starts in is billed in the run that covers that start.
    - Part periods are prorated by days, as a quantity fraction.
    - Usage quantities from datasets are left for later.
  - **Projects:** a fixed fee once the project is completed; milestones once reached. Projects gain a fee amount (also the not-to-exceed cap), and milestones gain an amount.
- **Invoices** are drafts until approved.
  - Editing flags lines changed by hand, the signal invoice review (next) uses.
  - Removing a line, or voiding the invoice, releases its sources.
  - Held drafts can't be approved.
  - Approval sets the issue and due dates.
  - Approved invoices are marked sent, by hand until the accounting sync lands, and are then locked.
  - Credit memos negate chosen lines of a sent invoice.
- **Tax** is a code per line for the accounting system to calculate. Huxley doesn't compute tax.

## Consequences

- Invoice approval (B2) adds rules, the review workflow, comments and the send gate on top of this lifecycle, using the quote review model.
- The accounting sync (B3) replaces "mark sent" with a sync that records its status, and syncs credit memos the same way.
- Usage-based recurring quantities, and expenses (Phase 12), become more candidate sources.
