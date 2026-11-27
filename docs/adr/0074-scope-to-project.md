# ADR-074: A won deal's project is laid out from its scoped work breakdown, and the scope tracks estimate against actual

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Presales estimates (ADR-073) end at the quote, but Phase 10 also says what happens next. On a win, the work breakdown becomes the project and the bill of materials feeds the sales order. Estimates are compared with actuals when the project closes, to improve the next estimate.

## Decision

- **Projects can be laid out from a structure** (a template's shape) given at creation, not only from a stored template. The budget defaults to the structure's phase budgets, else its tasks' estimates.
- **The sales order's project hand-off** (ADR-069) works as follows:
  - With no template picked, it builds the structure from the opportunity's scope: phases in work-breakdown order, tasks with their expected hours including contingency, and milestones from the tasks marked as milestones.
  - A template, when picked, still wins.
- **The bill of materials** already reaches the sales order: assembly puts it on the quote (ADR-073), and the order copies the accepted quote's lines.
- **Estimate against actual:** the scope reads the project its deal's sales order started, and shows each task's estimated hours against the hours logged on it. Drafting the next estimate already reads past projects' actual hours.

## Consequences

- Phase 10's slices are all in place. Billing the fulfilled orders and the projects is Phase 11.
- Matching estimate to actual is by task, so it works best when the project keeps the work breakdown's task names.
