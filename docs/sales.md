# Sales

Turn on **Sales and quoting** (Settings → Modules). It needs CRM and Contracts. **Pipeline** and **Leads** then appear in the **Business** space for anyone with `sales:read`. The built-in **Sales** role works leads and opportunities; read-only users can look (ADR-063).

## Leads

**Business → Leads** holds people who may buy, before there's a deal: the company, a person, how to reach them, where they came from, and notes. Forms, workflows and the API can capture them too (`POST /api/v1/leads`).

- **Convert** makes the client (a new client organization with relationship *prospect*, or an existing client you pick), a contact from the lead's person, and an opportunity, and takes you to the opportunity.
- **Disqualify** marks a lead not worth pursuing, with a reason. Setting its status back to *Working* picks it up again.

## The pipeline

**Business → Pipeline** shows open opportunities by stage. Drag a card to another stage to move it, or onto **Won** or **Lost** to close it with a reason. The header adds up one-time and monthly value and the weighted pipeline. **List** shows any status, filterable and searchable by name, client or number (`OPP-12`).

An opportunity's page has its details, the stage strip (click a stage to move it), **Won** and **Lost**, its value (one-time, monthly, first year, weighted) and its stage history.

- **Probability** comes from the stage unless you set one.
- **Weighted value** is (one-time + 12 × monthly) × probability.
- **Forecast** puts it in pipeline, best case or commit (or leaves it out).

## Settings

**Settings → Sales pipeline** (`sales:configure`) keeps the stages (order, kind and chance of winning) and the reasons deals are won and lost. A stage with open opportunities can't be removed until they move.

## Events

`sales.lead.created`, `sales.lead.converted`, `sales.opportunity.created`, `sales.opportunity.updated`, `sales.opportunity.stage_changed`, `sales.opportunity.won` and `sales.opportunity.lost`, for workflows: tell the account manager when a deal is won, start onboarding, post to a channel.
