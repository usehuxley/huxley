# Expenses

The expenses module covers what people spend and the mileage they drive for work (ADR-079).

## Logging expenses

Log an expense from a ticket's or project's **Expenses** card, or from **Business → Expenses**. Each one has:
- a **category**;
- the **day** (within the past year);
- what it was for;
- either the **amount, tax included**, with the tax and vendor; or, for **mileage**, the distance.

Mileage is the distance times the **mileage rate** in effect on that day, per mile or kilometer.

An expense goes against one of:
- an open **ticket** (its client, and its project if the ticket is a project task);
- a **project**, and optionally one of its phases;
- a **client**;
- your MSP, as an **internal** expense.

Add the **receipt** when you log it, or later: a photo (JPEG, PNG, GIF, WebP or HEIC) or a PDF.

Each expense is either **billable** (charged to the client, marked up) or not, and **reimbursable** (paid back to you) or not. The category sets both by default, and you can change them on each entry.

Two **policy flags** show on entries:
- **Over the limit**, when the amount is more than the category's limit.
- **No receipt**, when the category needs one and there isn't one yet.

You can change or delete your expenses while they're drafts, or after they've been sent back. Ticket and project pages show everyone's expenses there, with what was spent and what's to be billed.

## Reading receipts

With the **Receipt reading** pack installed (from **Packs**), adding a receipt has your AI provider read it (ADR-080). It reads the total, the tax, the currency, the date and the vendor.

What it read shows under the entry. **Use these** takes it into your draft, and nothing changes until you do. Anything it couldn't read is left as you entered it. When a receipt can't be read, the entry says so.

The `expense.receipt_added` event triggers the reading, and the `core.expense.read_receipt` step does it.

## Reports and approval

On **Business → Expenses**, **Submit for approval** bundles your drafts, and any expenses sent back to you, into an **expense report** (ADR-081). One report covers one MSP.

With the **Expense approval** pack installed, the report goes to the leads of your teams (administrators, if your teams have no lead). They see the totals, what's to be paid back, and any policy flags, and decide on the report's page:
- **Approve** locks its expenses.
- **Ask for changes** sends them back to you to change and submit again.
- **Reject** sends them back too, and closes the report.

You're told either way. Comments on a report reach whoever submitted it, and anyone @mentioned.

The events are `expense_report.submitted`, `expense_report.approved`, `expense_report.changes_requested` and `expense_report.rejected`. The `core.expense.report_decide` step records a decision.

## Billing and project cost

Once their report is approved, **billable** expenses go on the client's next invoice (ADR-082):
- Each expense is one line ("Travel and lodging: Hotel (2026-11-20)"), at its amount marked up.
- Each is billed once. Voiding the invoice makes it billable again.
- Expenses on fixed-fee or milestone projects are part of the fee and aren't billed separately.
- **Business → Billing** shows unbilled expenses beside time, products, recurring charges and projects.
- In the accounting sync, expense lines map like the other kinds (`kind:expense`, or the default item).

A project's **Billing** card has an **expense budget**. The project shows what's been spent: expenses submitted, approved or paid back, against that budget.

## Paying expenses back

On an approved report's page, someone with `expense:approve` can do either (ADR-083):
- **Send to accounting.** This creates a **bill** to the person, as a supplier, in QuickBooks Online or Xero. It has one line per expense to be paid back, on its category's **expense account**, and the report shows the bill and a link to it.
  - First, under **Billing → Accounting**, match people to suppliers (**People (suppliers)**) and categories to expense accounts (**Expense accounts**). A default account covers any category without its own.
  - Expenses in more than one currency, or an unmatched person or category, stop it with the reason.
- **Mark paid back.** This marks the report and its expenses as paid back.

**Expense reports → Export approved expenses (CSV)** downloads everything to be paid back from approved reports, for payroll or a bank upload.

The events are `expense_report.sync_requested` (for the sync packs), `expense_report.synced`, `expense_report.sync_failed` and `expense_report.reimbursed`. Sync packs report back with `core.expense.sync_result`.

## Card charges

**Expenses → Card charges** lists company card charges (ADR-084).

Someone with `expense:approve` imports them from the bank's CSV file:
- Required columns: date, description and amount.
- Optional columns: merchant, currency, card, transaction id, and the cardholder's email.

Charges imported before (by their transaction id) are skipped. Imports can also come through the API (`POST /api/v1/card-transactions/import`), for example from a bank integration's workflow.

Each new charge is matched to the cardholder's expense when one expense has the same amount and currency within three days and no charge yet.

Cardholders see their own charges. **Log as my expense** turns an unmatched charge into an expense that isn't to be paid back, since the company paid. A charge can also be **ignored**, and a match can be undone. The `card_transactions.imported` event carries how many were imported and matched.

## Reporting

The prebuilt **Expenses** dashboard (with `expense:read`) shows:
- what was spent in the range, what's to be billed, and what's to be paid back;
- spending by person, category, client and project.

The `expenses` analytics source has the details.

## Settings

In **Settings → Expenses** (`expense:configure`):
- **Categories:** each sets whether it's mileage or an amount; whether it's billed by default, and its markup; whether it's paid back by default; the limit over which entries are flagged; and whether a receipt is needed.
- **Mileage rates:** per mile or kilometer, in a currency, from a day. A new rate from a later day takes over from then.

Huxley comes with these categories:
- Mileage (billable);
- Parts and supplies;
- Travel and lodging;
- Meals;
- Software and subscriptions;
- Other.

The starting mileage rate is 0.70 USD a mile.

## Permissions

| Permission | What it allows |
| --- | --- |
| `expense:write` | Log your own expenses and mileage, and change your drafts. Technicians have it. |
| `expense:read` | See everyone's expenses. |
| `expense:approve` | Approve and reject expense reports. |
| `expense:configure` | Keep categories, limits and mileage rates. |
