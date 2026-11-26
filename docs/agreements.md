# Agreements

An **agreement** (the `contracts` module) records what a client has agreed to: what it covers, the rates for billable time, and the recurring fee. Huxley uses agreements to work out whether each time entry on a ticket is covered or billable (ADR-039). Agreements are on the client's **Agreements** tab.

## Kinds

- **Managed services:** a fee covers some kinds of work, optionally up to a number of hours each calendar month. Work beyond the hours is overage, billed at the rates.
- **Block hours:** the client buys a block of hours for the term, and covered work draws it down.
- **Time and materials** and **project:** all billable work is billed at the rates.

## What an agreement holds

- **Name, status and term.**
  - The status is *draft*, *active* or *cancelled*. The term has a start and an optional end.
  - An agreement applies on a day when it's active and the day is within its term.
- **Coverage**, for managed and block hours:
  - All work types, or chosen ones.
  - Optionally, kinds of asset. Time is then covered only on tickets linked to an asset of one of those types, which needs the Assets module.
  - Hours included a month (managed; empty means unlimited), or the hours in the block.
- **Rates:** a default hourly rate, and rates for particular work types.
- **A billing increment** in minutes. Billable time on each entry rounds up to it.
- **Recurring lines:** catalog products (from this client's catalog or one above it, in the agreement's currency) or free text. Each has a quantity, a price (empty means the product's price) and a billing cycle. Billing will turn them into invoice lines.
- **An SLA policy** for the client's new tickets (see [SLA](#sla)).
- **A priority**, renewal settings and notes.

Drafts can be deleted. Agreements that have been active are cancelled instead, so time billed under them stays explained.

## How time is evaluated

Each entry on a ticket falls under **one** agreement, or none:

1. Huxley looks at the agreements that apply on the entry's day, at the ticket's client or an organization above it, such as your MSP. The nearest comes first, then the lowest priority number, then the oldest.
2. The first one that covers the entry (its work type, and asset types if set) is used.
3. Otherwise, the first time-and-materials or project agreement bills it.
4. Otherwise, the first agreement is used, and the work is outside its coverage.

Each entry then gets an **outcome**:

| Outcome | Meaning |
|---|---|
| Covered | Within coverage and the allowance |
| Partly covered | The allowance ran out during it: the rest is billable |
| Overage | Covered work, but the allowance is used up |
| Billable | Outside coverage, or a time-and-materials or project agreement |
| Not billable | The entry is marked non-billable |
| No agreement | No agreement applies |

**Allowances are used in order:** by the day worked, then by when the entry was logged. Logging earlier work later moves it ahead of later work. Billable minutes round up to the increment, and the amount is the hours times the work type's rate, or the default rate.

**Outcomes are worked out each time you look,** so edits to entries, agreements, rates or asset links show straight away. When billing arrives, invoicing will keep the outcomes it billed.

**Where outcomes show up:**
- On a ticket, each time entry shows its outcome, its agreement, and the billable time and amount.
- On the client's Agreements tab, each active agreement shows this month's covered and billable time and amount, entry by entry.

## A typical setup

- A **time-and-materials agreement at your MSP** with your standard rates bills any client's work that nothing else covers.
- A **managed agreement on each managed client** covers the work types in their plan.

## Permissions

- `agreement:read` at the client's organization: see its agreements, their usage, and how its tickets' time falls under them. Technicians and read-only users have it.
- `agreement:manage`: add, change, cancel and delete agreements.

Changes are audited (`agreement.created`, `.updated`, `.deleted`).

## API

- `/api/v1/agreements`:
  - `GET` lists, filtered by `orgId`.
  - `POST` takes `{ "orgId", "agreement": { … } }`.
  - `GET`, `PUT` (with the `version` you read) and `DELETE` work on `/{id}`.
- `GET /api/v1/agreements/{id}/usage?from=&to=`: the entries that fell under it, with totals (default: this month).
- `GET /api/v1/tickets/{id}/time-coverage`: each time entry on a ticket, evaluated.

## SLA

An agreement can name an **SLA policy**. While it's in force, new tickets at its client, and at organizations below, get that policy.
- **Against assignments:** it applies unless a policy is set on an organization nearer the ticket. On the same organization, the agreement wins.
- **Which agreement:** when several agreements bind policies, the nearest wins, then the lowest priority number.
- **Existing tickets** keep the terms they were opened under.

## Renewals

- **Notice:** once per term, when an active agreement is within its **renewal notice** (30 days by default) of its end date, Huxley announces `contract.renewal_due`. The event carries the agreement, its end date, days left and whether it renews automatically. Workflows can use it to remind account managers or open a renewal.
- **Automatic renewal:** an agreement that **renews automatically** rolls on to a new term of the same length the day after it ends. Its allowances start afresh. Huxley announces `contract.renewed`, and the renewal is audited.

## Billing previews

An agreement's usage can count only time on approved timesheets: pass `approvedOnly=true`, or turn on **Bill only approved time** in the timesheet settings (see [Timesheets](workforce.md#timesheets)).
