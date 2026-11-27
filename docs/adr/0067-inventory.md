# ADR-067: Inventory is a ledger of movements over stock per item, location and lot

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Phase 10 calls for inventory: warehouses and locations (including technicians' vehicles), stock levels, serial and lot tracking, movements (receive, transfer, allocate, consume, return, RMA), allocation to tickets and projects, and reorder alerts (`stock.low`). Purchase orders and sales orders come next and will receive into and allocate from it.

## Decision

- **Who owns stock:** the organization that holds it, usually the provider. Locations, items and every stock table are organization-scoped (RLS). An **item** is a catalog product an organization stocks. It's unique per organization and product, and keeps a copy of the product's SKU, name and unit. The product can belong to an ancestor organization, which the stocking organization's staff can't see under RLS, so the copy keeps lists readable.
- **Tracking** per item is by quantity, by serial number, or by lot.
  - Serial items move whole, named units. The count follows from the serials. Each unit is a row with its status: in stock, allocated, consumed, RMA or written off.
  - Lot items name the lot on every movement. Stock is kept per item, location and lot (lot is empty when untracked).
- **Stock rows** hold *on hand* and *allocated*. Every change goes through one function that locks the row and refuses to go below zero, or to allocate more than is on hand. Empty rows are removed.
- **The ledger:** each movement is appended to `inventory_movements` and never changed. It's also audited and raised as an `inventory.*` event. Hand-made movements are receive, transfer, adjust (signed, with a reason), return and RMA.
- **Allocations** set parts aside for a ticket or project, which must be one the caller can see. They're then **consumed**, leaving stock with the last received unit cost for job costing, or **released**. "Used already" allocates and consumes in one step.
- **Reorder points** are per item and location. When available stock falls to the point, `stock.low` is raised once; a `low_since` mark stops repeats until stock recovers.
- **Permissions:** `inventory:read`, `inventory:write` and `inventory:configure`, at the organization holding the stock. Technicians read and write, so they can take parts for their work.

## Consequences

- Purchase orders (next) receive through the same movement function with a PO reference. Sales orders allocate the same way.
- Valuation (FIFO or average cost) isn't kept yet. Received unit costs are in the ledger, so it can be computed later.
- Moving stock between organizations isn't supported. Transfers stay within the holding organization.
