# ADR-069: Accepted quotes become sales orders, fulfilled through inventory and purchasing and handed off by the API

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Phase 10's sales orders run from an accepted quote through purchase orders, receiving, allocation and fulfilment. They then hand off to agreements (recurring lines), projects (created from a template) and, in Phase 11, invoices.

## Decision

- **Made on acceptance.** Accepting a quote, staff-recorded or signed in the portal, creates its sales order in the same transaction. The order is at the client, one per quote, and copies the lines agreed to: items, and options only if taken. A migration backfills orders for quotes accepted earlier.
- **Lines draw on stock** through the nearest organization above the client that stocks the product. That organization is resolved on read, not stored, so stocking a product later still works.
  - What a line has **set aside** is the open allocations to the order for that item.
  - What it has **on order** is what's outstanding on purchase order lines bought for the order.
  - Both are per item: two lines of one product share them.
- **Inventory and purchasing gain a third reference type**, `sales_order`, beside ticket and project. It's used for allocations and for purchase order lines bought for something.
- **Fulfilment steps:**
  - Allocate from the locations with the most available. Serial units are picked automatically.
  - Draft purchase orders per vendor for what's short (vendor matched as for reorders).
  - Deliver by consuming allocations. Lines with nothing to draw on (services, unstocked products) are delivered with the order. Recurring lines are delivered by the agreement.
- **Hand-offs are composed in the API layer**, which already depends on contracts and projects; the sales crate doesn't. The API builds the agreement from the recurring lines and creates the project, then records each on the order. It checks `agreement:manage` and `project:manage` at the client.
- **Fulfilled** when every line is delivered. That raises `sales_order.fulfilled`, which Phase 11 billing will use to invoice. After that the order is closed. Cancelling an open order with nothing delivered releases its allocations.

## Consequences

- Billing in Phase 11 reads fulfilled orders' one-time lines, and the agreement for recurring ones.
- Partial deliveries of a line are supported through its allocations. There's no per-line, per-delivery record beyond the inventory ledger yet.
- Presales scoping (S10) will add its bill of materials as quote lines, so its output flows through the same order.
