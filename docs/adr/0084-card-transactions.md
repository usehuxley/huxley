# ADR-084: Card charges are imported, deduplicated by bank ID, and matched to the cardholder's expenses

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

§8.16: card transactions are imported (by CSV or through integrations) and matched to entries. Events support automation, and expense dashboards cover person, client, project and category.

## Decision

- **Charges** belong to an MSP. Each has an optional cardholder (a user), the day, description, merchant, amount, currency, the card's last four digits, and the bank's ID.
  - The bank's ID is unique per MSP, so importing twice skips what's there.
  - Each charge can be matched to one expense, and each expense to one charge.
- **Importing** is a JSON API: rows, up to 5,000 at a time.
  - The web app reads the bank's CSV, finding columns by common names and cardholders by email, and posts rows. Huxley needs no CSV dependency.
  - Integration workflows post rows the same way.
  - Importing needs `expense:approve` at the MSP.
- **Matching:**
  - **Automatic**, on import: a single free expense of the cardholder with the same amount and currency within three days. Ambiguous charges are left for a person.
  - **By hand**: match, unmatch or ignore.
  - **Log as my expense**: the cardholder turns a charge into a non-reimbursable expense (the company paid), matched to it.
- **Visibility:** cardholders see their own charges. `expense:approve` and `expense:read` see everyone's.
- **Reporting:** an `expenses` analytics source and a prebuilt **Expenses** dashboard (spent, to bill, to pay back; by person, category, client and project). Drafts are left out.

## Consequences

- A bank integration pack only needs a workflow that reads transactions and posts them to the import API.
- Unmatched charges for a cardholder are a natural prompt for a reminder workflow, for example on `card_transactions.imported`.
