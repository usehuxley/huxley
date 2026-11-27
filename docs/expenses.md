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
