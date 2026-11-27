# ADR-063: Leads live at the provider, opportunities at the client; stages are instance-wide

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 10 starts sales (design doc §8.10) with leads, opportunities, a configurable pipeline and win/loss reasons. Quotes, orders, forecasting and presales scoping build on it. Records need a home in the organization tree so RLS and inherited permissions work as they do everywhere else.

## Decision

- **A lead belongs to the provider organization that captured it.** A prospect isn't a client organization yet, and many never become one. Leads hold the company name, a person, a source, notes, an owner and a status (new, working, disqualified, converted).
- **Converting a lead** makes, in one transaction, the client (a new client organization under the lead's, with CRM relationship *prospect*, or an existing client), a contact from the lead's person, and an opportunity. The lead keeps links to all three. A company name already used by a sibling client is refused with a hint to convert into that client.
- **An opportunity belongs to the client it would sell to**, so the client's people, assets and history sit beside it and access follows the client. It carries a one-time amount, a monthly recurring amount, a currency, an expected close date, a forecast category (pipeline, best case, commit, omitted), an owner, a contact and a source.
- **Stages are instance-wide** (like ticket types): ordered, each open, won or lost, with a probability (won 100, lost 0). There is always at least one of each kind. Moving to a won or lost stage closes the opportunity with an optional reason of that kind; moving back to an open stage reopens it. Every move is kept in the stage history. A stage in use is archived, not deleted.
- **Probability** is the stage's unless overridden. **Weighted value** is (one-time + 12 × monthly) × probability.
- **Events:** `sales.lead.created`, `sales.lead.converted`, `sales.opportunity.created`, `.updated`, `.stage_changed`, `.won` and `.lost`.
- **Permissions:** `sales:read`, `sales:write` (converting also needs `crm:manage` where the lead is) and `sales:configure` (instance: stages and reasons). A system **Sales** role holds sales read and write with CRM and catalog access; read-only users can read sales.

## Consequences

- Quotes attach to opportunities and inherit their client scope.
- Forecasting reads stages, probabilities, categories and close dates without new tables.
- Instance-wide stages mean one pipeline per instance; per-team pipelines would be a later addition.
