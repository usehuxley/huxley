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

## Quotes

**Business → Quotes**, or **New quote** on an opportunity, starts one. The quote page is the builder:

- **Add from the catalog** fills in the SKU, name, unit, cost and price; or add a **line** of your own, or a **section** heading. Lines can be one-time or recurring (monthly, quarterly, annual), with a quantity, unit price, discount and unit cost. Tick **Option** for something the client may take or leave; options count once the client takes them.
- **Totals** update as you type: one-time, monthly, the first year (one-time + 12 months) and the **margin** on it. Options count once the client takes them.
- **Save** keeps the quote. **Mark sent** records that it went out and keeps what was sent as a **revision**. Changing a sent quote starts the next revision; send it again when it's ready.
- **Accepted** or **Declined** records the client's answer: who, and the options taken or why not. Accepting wins its opportunity.
- **PDF** downloads the quote with its sections, options and totals.

### Clients answer in the portal

A sent quote shows in the client portal under **Quotes** for its contact, and for the company's approvers and admins (ADR-065). They see it as sent, without costs, tick the options they want, type their name, **sign** on the pad and accept, or decline and say why. The quote's page then shows the signature, who signed, when, from where, and a fingerprint (SHA-256) of the revision they agreed to. Accepting wins the opportunity. A quote past its *valid until* date can't be accepted.

### Internal review

Review rules (below) decide which quotes need a second look before they go out: every quote, a margin below a percent, a first year above an amount, a line discounted more than a percent, a client's first quote, a product category, or an expression over the quote. A quote that matches one shows **Needs review** and why, and **Mark sent** gives way to **Submit for review** (ADR-066).

- **Submit for review** sends it to the rule's reviewers through the **Quote review** pack (install it from **Packs**, then publish its workflow). Reviewers approve, **request changes** or reject on the quote page or under **Approvals**, with a comment; the quote's owner is told.
- **Changed since the last review** lists the lines added, removed and changed, and the totals, so reviewers see only what's new.
- Once approved, it can be sent. Changing it afterwards needs a new review.
- **Send without approval** (administrators, `sales:review:override`) sends it anyway; the quote and the audit log record that.
- **Comments** go on the whole quote or a line. **Mention someone** to tell them; reply in the thread, and **Resolve** it when settled. Lines with open comments say so in the builder.

## Settings

**Settings → Sales pipeline** (`sales:configure`) keeps the stages (order, kind and chance of winning), the reasons deals are won and lost, and the **quote review** rules with who reviews each (`role:<key>`, `team:<id>` or `user:<id>`). A stage with open opportunities can't be removed until they move.

## Events

`sales.lead.created`, `sales.lead.converted`, `sales.opportunity.created`, `sales.opportunity.updated`, `sales.opportunity.stage_changed`, `sales.opportunity.won`, `sales.opportunity.lost`, `sales.quote.created`, `sales.quote.sent`, `sales.quote.accepted`, `sales.quote.declined`, `quote.submitted_for_review`, `quote.approved`, `quote.changes_requested` and `quote.rejected`, for workflows: tell the account manager when a deal is won, start onboarding, post to a channel.
