# ADR-073: Estimates are three-point hours by priced role plus a sized bill of materials, drafted by the scoping pack and assembled into the quote

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Presales scoping (ADR-072) needs the work breakdown and bill of materials it promises:
- low, likely and high hours by role, grounded in templates and past actuals, with contingency by risk;
- quantities from the client's environment, matched to the catalog;
- quote assembly with consistency checks.

## Decision

- **Roles are catalog services priced per hour.** A role is a product of kind service whose unit is an hour, so labour on the quote is an ordinary priced line and no separate rate card is needed yet.
- **Tasks** carry a phase, a role, low, likely and high hours, an optional milestone flag, and the deliverable they serve. Expected hours use the three-point (PERT) estimate, (low + 4 × likely + high) ÷ 6. The scope's **risk** adds 10, 20 or 35% contingency.
- **Materials** are catalog products, or named lines when nothing matches. Each has a quantity, its basis in words, and an alternate flag (alternates become quote options).
- **Drafting** is a second workflow in the Presales scoping pack, triggered by `scope.estimate_requested`.
  - `core.scope.estimate_context` gives the AI the Scope of Work, the client's environment, the providers' project templates (phases, tasks, estimate hours), and the average estimate and actual logged hours of past completed projects' tasks.
  - It also gives numbered lists of roles and products, which the AI picks from by number.
  - `core.scope.write_estimate` keeps rows people edited and replaces the AI's earlier rows.
- **Checks** before quoting:
  - the scope isn't approved, or open questions remain;
  - tasks have no priced role, or a role has no price;
  - materials aren't matched to the catalog;
  - a deliverable in the Scope of Work has no task serving it (text match).
- **Assembly** creates a draft quote on the opportunity: a Labour section (hours per role with contingency, rounded up to the half hour) and a Hardware and software section, carrying the latest approved Scope of Work. It then goes through review and sending like any quote.

## Consequences

- Turning the WBS into the project on a win (project tasks from the scope's tasks) and the BOM into the sales order are next. The sales order already takes the quote's lines.
- Comparing estimates with actuals when a project closes can reuse the same task titles and logged time.
