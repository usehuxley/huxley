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

## Activities

Leads and opportunities have an **Activity** card (ADR-070). **Log or plan something** adds:

- a **task**, with when it's due and who it's for (you, by default). Someone given a task is told.
- a **call**, with how long it took and how it went.
- a **meeting**, with when it is; tick *It's happened* once it has.
- a **note**.

Open tasks sit at the top until they're marked **Done**. **Business → Activities** lists your tasks and meetings: overdue, today and coming up, plus what you finished lately.

### Logging email

Set a mailbox's **What it's for** to *Sales email* (**Settings → Mailboxes**), install the **Sales email logging** pack, and publish its workflow. Then:

- **BCC the mailbox's address** on mail to clients and prospects, or point an IMAP or Microsoft 365 mailbox at it to sync.
- Each message is filed as an email activity on every contact it's from or to, on the contact's client's most recently changed open opportunity, and on open leads with those addresses.
- Addresses that match no one are left out, and a message is logged once.
- A sales mailbox never opens tickets.

## Forecast

**Business → Forecast** shows a month, quarter or year. For each salesperson it shows, in first-year values (one-time plus twelve months of recurring):

- **Quota:** what they're expected to win.
- **Won:** deals won with their close in the period, and the percentage of quota.
- **Commit**, **best case** and **pipeline:** open deals expected to close in the period, by forecast category. Best case includes commit, and pipeline includes both. Deals left out of the forecast don't count.
- **Weighted:** the open deals' value times their probability.

Sales configurers (`sales:configure`) **set quotas** for a month or a quarter, from the forecast. Set them one way, monthly or quarterly, not both. A year shows the total of the quotas starting in it. The **Sales** dashboard (**Dashboards**) charts the pipeline and wins, and the `opportunities`, `quotes` and `sales_activities` sources are there for your own dashboards (see [Reporting data](analytics.md)) (ADR-071).

## Orders

Accepting a quote, whether in Huxley or by the client signing in the portal, makes a **sales order** (SO-*n*) at the client, with the lines agreed to: the options taken, not the rest. **Business → Orders** lists them; the quote links to its order (ADR-069).

The order page shows each line's state: short, on order, ready, delivered, or waiting for the agreement. From there:

- **Set aside stock** allocates what's in stock for the one-time product lines. It draws from the nearest organization above the client that stocks the product, and from the locations with the most first. This takes `inventory:write`.
- **Order what's short** drafts purchase orders, one per vendor, for what's still missing. The lines are bought for the order and set aside for it when they arrive. Products whose catalog vendor matches no vendor are listed so you can order them by hand. This takes `procurement:write`; see [Purchasing](purchasing.md).
- **Deliver what's ready** uses what's set aside: it leaves stock and counts as delivered. Services and unstocked lines are delivered with the order.
- **Make the agreement** hands the recurring lines to a new agreement for the client, as its billing lines. The agreement starts as a draft, unless you make it active. This takes `agreement:manage`.
- **Start a project** creates the client's project, laid out from a project template if you pick one. This takes `project:manage`.

When every line is delivered, the order is **fulfilled**, and `sales_order.fulfilled` is raised for billing. **Close** it then. **Cancel order** works on an open order nothing has been delivered from. It gives back the stock set aside for it; purchase orders made for it stay as they are.

## Settings

**Settings → Sales pipeline** (`sales:configure`) keeps the stages (order, kind and chance of winning), the reasons deals are won and lost, and the **quote review** rules with who reviews each (`role:<key>`, `team:<id>` or `user:<id>`). A stage with open opportunities can't be removed until they move.

## Events

`sales.lead.created`, `sales.lead.converted`, `sales.opportunity.created`, `sales.opportunity.updated`, `sales.opportunity.stage_changed`, `sales.opportunity.won`, `sales.opportunity.lost`, `sales.quote.created`, `sales.quote.sent`, `sales.quote.accepted`, `sales.quote.declined`, `quote.submitted_for_review`, `quote.approved`, `quote.changes_requested`, `quote.rejected`, `sales.activity.created`, `sales_order.created`, `sales_order.fulfilled`, `sales_order.closed` and `sales_order.cancelled`, for workflows: tell the account manager when a deal is won, start onboarding, post to a channel.
