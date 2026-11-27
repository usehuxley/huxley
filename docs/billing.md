# Invoicing

The billing module turns what Huxley tracks into invoices (ADR-075). Huxley decides **what to bill**. Your accounting system remains the record for posted invoices, payments and tax.

## What gets billed

A **billing run** looks at a period and finds, for each client:

- **Time:**
  - Billable time worked in the period, after agreement coverage. Covered time isn't billed; overage and time outside coverage are.
  - The rate is the agreement's, or your **default hourly rate**.
  - A ticket's time at one rate becomes one line.
  - Time on fixed-fee or milestone projects isn't billed hourly. Time on a not-to-exceed project stops at its cap.
  - With **Bill only time on approved timesheets** on, only approved weeks count.
- **Products:** sales order lines delivered in full.
- **Recurring charges:**
  - Active agreements' lines, for each calendar month, quarter or year that starts in the period.
  - The month an agreement starts in is prorated by days ("16 of 31 days"), as is the month it ends in.
- **Projects:**
  - A fixed-fee project's fee, once the project is completed.
  - A milestone project's milestones, once reached, each at its amount.
  - Set fees and milestone amounts on the project page, in its **Billing** card.

Each source is billed once. Voiding an invoice makes its sources billable again.

## Billing runs

**Business → Billing** shows what's unbilled in a period, by client: time, products, recurring and projects. Tick clients and choose **Bill**, or bill everyone. Each client gets a **draft invoice** in each currency, and nothing is sent yet.

## Invoices

**Business → Billing → Invoices** lists them. A draft invoice can be:

- **edited:** change descriptions, quantities and prices, or add lines. Lines changed by hand are flagged. Removing a line makes what it billed billable again.
- **held**, with a reason. A held invoice can't be approved until it's released.
- **approved** (`billing:approve`). The issue date is set, and the due date follows your **payment terms**.
- **voided**, which makes what it billed billable again.

An approved invoice is **marked sent**, by hand for now (accounting sync comes next), and is then locked. To correct one that's gone out, make a **credit memo**. It negates the lines you pick (all, by default) and goes through the same approval.

Every invoice has a **PDF**.

## Review

No invoice goes to a client or the accounting system until it's approved. **Review rules** decide which invoices need an internal review first, and who reviews them. Out of the box, an invoice with a line **changed by hand** needs review by administrators. Other rules can match:
- every invoice;
- a total above an amount;
- a total that moved more than a percent from the client's last invoice;
- a client's first invoice;
- credit memos;
- an expression over the invoice, e.g. `invoice.total | float > 10000`.

Keep rules in **Settings → Billing** (`billing:configure` at the top).

A draft that needs review shows why on its page. **Submit for review** sends it to the **Invoice review** workflow; install the pack from **Packs** first. The reviewers approve it, ask for changes or reject it, and an approved review approves the invoice. Whoever submitted it is told.

When an invoice changes after review, it needs a new one, and the reviewer sees what changed since the last decision. Someone with `billing:review:override` can approve an invoice without review. The override is recorded and shown on the invoice.

Comment on the invoice or on a line, and **@mention** people to tell them. Resolve a thread once it's settled.

### Approving a run

On a run's invoices (**Billing runs → Invoices**), **Approve the run** approves every draft that needs no review. Invoices on hold or needing review are listed as exceptions. Optionally, those that need review are submitted for review in the same pass.

## Accounting sync

Approved invoices and credit memos go to your accounting system (ADR-077). Huxley prepares each one with your matches. The accounting system's pack (**QuickBooks Online**, or **Xero**) creates it there through a system workflow, so other systems can be added the same way.

1. Install the pack from **Packs** at your MSP, connect it, and run its **Fetch lists** workflow once. After that it runs nightly.
2. In **Settings → Billing → Accounting sync**, set the accounting system (`quickbooks` or `xero`). Choose whether invoices sync **as soon as they're approved**, or only when you sync them.
3. In **Business → Billing → Accounting**, match:
   - **customers:** each client (a client without its own match uses the nearest client above it);
   - **items:** products, kinds of line (time, recurring, project, product, manual) and a **default item** for the rest;
   - **tax codes:** each tax code on your lines, and one for lines without a code;
   - **classes** (tracking categories in Xero): a default, or per client. Optional.

The choices come from the lists the pack fetched. Without them, type the ID.

An invoice with an unmatched client or item fails straight away, saying what to match. The invoice page's **Accounting** card shows each invoice's sync: syncing, synced (with its number there and a link to open it), or failed (with the accounting system's reason).

Failed syncs are listed under **Accounting → Sync errors**. Fix the cause and **Retry**. Once synced, an approved invoice is sent.

Events: `invoice.sync_requested` (the prepared invoice, for sync packs), `invoice.synced` and `invoice.sync_failed`. A sync pack's workflow reports back with `core.invoice.sync_result`.

## Settings

**Settings → Billing** (`billing:configure`), per provider, sets:
- the default hourly rate;
- the currency;
- payment terms in days;
- whether only approved timesheets are billed.

## Events

`invoice.approved`, `invoice.sent` and `invoice.voided`, for workflows. For review: `invoice.submitted_for_review` (with the reasons, the reviewers, what changed and the client's previous total), `invoice.review_approved`, `invoice.changes_requested` and `invoice.rejected`. The `core.invoice.review` step records a decision.

## Permissions

| Permission | What it allows |
| --- | --- |
| `billing:read` | See invoices, runs and what's unbilled. |
| `billing:write` | Run billing, edit, hold, void and mark invoices sent, and make credit memos. |
| `billing:approve` | Approve invoices. |
| `billing:review:override` | Approve an invoice that needs review without one (recorded). |
| `billing:configure` | Keep billing settings and accounting matches, and at the top, the review rules. |
