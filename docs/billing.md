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

## Settings

**Settings → Billing** (`billing:configure`), per provider, sets:
- the default hourly rate;
- the currency;
- payment terms in days;
- whether only approved timesheets are billed.

## Events

`invoice.approved`, `invoice.sent` and `invoice.voided`, for workflows.

## Permissions

| Permission | What it allows |
| --- | --- |
| `billing:read` | See invoices, runs and what's unbilled. |
| `billing:write` | Run billing, edit, hold, void and mark invoices sent, and make credit memos. |
| `billing:approve` | Approve invoices. |
| `billing:configure` | Keep billing settings. |
