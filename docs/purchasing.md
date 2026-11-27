# Purchasing

Purchasing is part of the inventory module (ADR-068). You keep **vendors**, raise **purchase orders**, get the larger ones approved, send them, and **receive** deliveries into stock, in full or in part.

## Vendors

**Business → Purchasing → Vendors** (`procurement:write`) keeps who you buy from:

- the orders email
- your account number with them
- payment terms and currency

A vendor with orders can't be removed. Turn it off instead (untick **In use**).

If a catalog product's **vendor** matches a vendor's name, low stock of that product is reordered from that vendor (see [Reordering](#reordering)).

## Purchase orders

**New order** picks a vendor and opens the order (PO-*n*). Add **stocked items**: their catalog cost fills in, and you can change it. You can also add lines of your own, such as shipping. Then choose where it's delivered and when it's expected.

An order moves through these stages:

1. **Draft:** the only stage where it can be changed or deleted.
2. **Submit:**
   - If the total is over your **approval threshold**, it waits for someone with `procurement:approve`. They **approve** it, or **send it back** with what to change, and it returns to draft.
   - Otherwise it's approved straight away.
3. **Mark ordered** once you've sent it. **PDF** downloads it to email to the vendor, and you can record their order reference.
4. **Receive** each delivery: the location it went to, how many of each line arrived (serial numbers for serial-tracked items, the lot for lot-tracked ones) and the delivery note. Stocked lines go into inventory as a *receive* movement, with the order's cost and reference. The order shows **Partly received** until everything has arrived.
5. **Close** a partly received order when the rest isn't coming. **Cancel** an order while nothing has arrived.

### Buying for a ticket or project

A stocked line can be bought for a ticket or project (`forRefType` and `forRefId` in the API). When it arrives, it's received and then set aside (allocated) for that job, so it shows on the job's **Parts** card ready to use.

## Approval threshold

**Settings → Inventory → Purchase order approval** (`procurement:approve`) sets, for each organization, the total above which orders need approval. Leave it empty to approve every order on submit.

## Reordering

**Reorder low stock** drafts one order per vendor and location for the items at or under a reorder point (see [Inventory](inventory.md#reorder-points)). It skips quantities already on open orders, and orders the reorder quantity, or enough to get back above the point. The API's `GET /api/v1/purchase-orders/suggestions` lists the same suggestions, including items with no matching vendor.

## Events

For workflows:

- `purchase_order.submitted` (waiting for approval), `purchase_order.approved`, `purchase_order.rejected`
- `purchase_order.ordered`, `purchase_order.partially_received`, `purchase_order.received`
- `purchase_order.cancelled`

For example, a workflow can ask an approver in Teams, email the PDF to the vendor, or tell the technician when their parts arrive.

## Permissions

| Permission | What it allows |
| --- | --- |
| `procurement:read` | See vendors and orders. Technicians and read-only users have it. |
| `procurement:write` | Keep vendors, draft, submit, send, cancel and close orders, and receive deliveries. |
| `procurement:approve` | Approve or send back orders over the threshold, and set it. |
