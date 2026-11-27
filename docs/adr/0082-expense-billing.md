# ADR-082: Approved billable expenses are billing sources; projects keep an expense budget

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

§8.16: billable expenses have markups and are evaluated against agreement coverage. Approved billable expenses become invoice sources in billing runs, marked billed once invoiced. Project cost budgets and burn include expenses.

## Decision

- **A new billing source.** Approved (or reimbursed) billable expenses, spent up to the end of the run's period, are added to a client's candidates:
  - one `expense` line each: "Category: what (day)", quantity 1, at the amount marked up by the entry's markup;
  - marked in `billing_billed` as `expense`, so they're billed once and voiding releases them.
- **Coverage.** Agreements don't describe expenses, so coverage is decided by the work the expense was for:
  - Expenses on fixed-fee or milestone projects are part of the fee and aren't billed.
  - Everything else is billed as the entry says (billable or not, with its markup).

  If agreements later cover expenses, the rule belongs in the coverage engine, as it does for time.
- **Unbilled totals** gain an `expenses` column. Accounting mappings accept `kind:expense`.
- **Projects** gain an expense budget (`cost_budget`), and show their expense cost (submitted, approved or reimbursed) against it, beside the hours budget.

## Consequences

- An expense the client should pay only shows on invoices once it's approved, so approval gates billing.
- Profitability reports can add expense cost to time cost.
