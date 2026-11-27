# ADR-068: Purchase orders are approved by a threshold and received through inventory movements

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Phase 10 calls for purchasing: vendors, purchase orders (manual, or from sales orders later), approval, receiving into inventory, and partial receipts. Inventory (ADR-067) already has a receive movement and allocations.

## Decision

- Purchasing lives in the **inventory module** ("Inventory and procurement") and the same crate. Vendors and orders are organization-scoped to the buying organization.
- **Lifecycle:**
  1. draft
  2. submitted (only when over the threshold)
  3. approved
  4. ordered
  5. partially received
  6. received

  An order can be cancelled while nothing has arrived. A partly received order can be closed when the rest isn't coming. Only drafts change, with a version check. Every step is audited and raised as a `purchase_order.*` event, so workflows can add notifications, extra approvers or email to the vendor.
- **Approval** uses a per-organization threshold on the order total and the `procurement:approve` permission. It isn't a workflow approval step. Purchase approval is usually one level, and a threshold is easy to explain. Organizations that want more can react to `purchase_order.submitted` in a workflow.
- **Lines** are either stocked items, which copy their SKU and name and default the cost from the catalog, or lines of their own, such as shipping or services. A stocked line can be bought for a ticket or project, which must exist when drafted.
- **Receiving** goes through inventory's movement function with the order as its source (`ref_type = purchase_order`), so stock, serial units, lots, the ledger and `stock.low` work as for any receipt. Lines bought for a job are then allocated to it. Each delivery is kept as a receipt row. Over-receiving a line is refused.
- **Reordering** suggests, for items at or under a reorder point, the reorder quantity (or enough to get back above the point) minus what's on open orders. The vendor is the one whose name matches the catalog product's vendor. One call drafts an order per vendor and location.

## Consequences

- Sales orders (next slice) can generate purchase orders through the same create function, with lines bought for the order.
- The PDF uses the shared export renderer and has no branding yet, the same as quotes.
- Matching vendors by name is simple but fragile. A product-to-vendor link with the vendor's SKU and cost can replace it when distributor integrations arrive.
