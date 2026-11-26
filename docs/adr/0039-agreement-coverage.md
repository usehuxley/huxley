# ADR-039: Agreements evaluate time on demand, one agreement per entry

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

MSPs bill clients under agreements (design doc §8.7):

- **Managed services:** a flat or per-user/device fee covers some kinds of work. Some agreements include a number of hours a month, beyond which work is billed.
- **Block hours / retainers:** the client buys hours up front, and work draws them down.
- **Time and materials:** all billable work is charged at agreed rates.
- **Project:** like time and materials, for a piece of work.

The Phase 6 exit criterion is that time entries on tickets evaluate correctly as covered or billable under an agreement. Timesheets, billing previews and invoices build on that.

## Decision

### An agreement

An agreement (`contract_agreements`) belongs to an organization, usually a client. It has:

- **A kind:** `managed`, `block_hours`, `time_materials` or `project`.
- **A status** (`draft`, `active`, `cancelled`) and a **term**: `starts_on` and an optional `ends_on`. It's **in force** on a day when it's active and the day falls within its term.
- **Coverage**, for `managed` and `block_hours`:
  - Which **work types** it covers: all billable ones, or a chosen set.
  - Which **asset types** it covers: any, or a chosen set. With a set, a ticket's time is covered only when the ticket is linked to an asset of one of those types.
  - **An allowance:** `managed` agreements may include minutes per calendar month (none: unlimited). `block_hours` agreements hold a number of minutes for their whole term.
- **Rates** (a rate card): an hourly rate per work type, and a default rate. Billable time is charged at its work type's rate, else the default.
- **A billing increment** (1 to 60 minutes): each entry's billable minutes round up to it.
- **Recurring lines**: catalog products (or free text) with a quantity and price, billed every cycle. They describe the agreement's fee. Billing (Phase 9) turns them into invoice lines, and evaluation doesn't use them.
- **A priority**, for when several agreements apply.

### Which agreement a time entry falls under

An entry on a ticket is evaluated against **exactly one** agreement, or none. The **candidates** are the agreements in force on the entry's day at the ticket's organization or above it. They're ordered by the nearest organization first, then lowest priority number, then oldest.

1. Use the first candidate whose coverage includes the entry, by work type and asset types.
2. Otherwise, the first `time_materials` or `project` candidate.
3. Otherwise, the first candidate: the work is outside its coverage.
4. With no candidates, there's no agreement.

### Outcomes

| Outcome | When | Billable minutes |
|---|---|---|
| `non_billable` | The entry is marked non-billable | 0 |
| `covered` | Within coverage and allowance | 0 |
| `partly_covered` | Within coverage, using the last of the allowance | The rest |
| `overage` | Within coverage, allowance used up | All |
| `billable` | Outside coverage, or a `time_materials`/`project` agreement | All |
| `no_agreement` | No agreement applies | All (no rate) |

- Billable minutes round up to the increment, and the amount is minutes ÷ 60 × rate, in the agreement's currency.
- **Allowances are consumed in order:** by the day worked, then when the entry was logged, then by id. That covers each covered entry in the month (managed) or the term (block hours).

### On demand, not stored

Outcomes are **computed when asked**: for one entry, for a ticket's entries, or for an agreement's usage over a period. They aren't stored.

- **Edits don't leave stale results:** changing an entry, the agreement, its rates or the ticket's assets just changes the next evaluation.
- **Results are reproducible:** the ordering above makes them deterministic.
- **Billing freezes them:** approving timesheets and invoicing (later phases) take a snapshot of the outcomes they bill, so what was billed stays as billed.

The cost is reading a month's or a term's entries for an allowance. That's indexed, and small at MSP scale.

## Consequences

- Agreements, coverage, rates and lines are new tables in the `contracts` module, with `agreement:read` and `agreement:manage` at the agreement's organization.
- The evaluation reads `sd_time_entries`, `sd_work_types` and ticket asset links (`cmdb_asset_tickets`). With Assets off, asset-type coverage can't be met, so the API refuses asset types then.
- Binding an SLA policy to an agreement, renewal events (`contract.renewal_due`), and billing previews are follow-ups.
