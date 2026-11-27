# Inventory

The inventory module keeps track of the parts your organization stocks: where they are, how many are free, which serial numbers you hold, and every movement in and out (ADR-067). Turn it on under **Settings → Modules** (it needs Agreements, for the product catalog).

## Locations

**Settings → Inventory** (`inventory:configure`) keeps the places stock is held:

- **Warehouses**, and the **bins** (shelves) in them
- **Vehicles**, each with the technician who drives it
- **Client sites**, for stock kept with a client

A location that has held stock can't be removed, so its history stays readable. Turn it off instead (untick **In use**).

## Items

**Business → Inventory** lists what's stocked, with what's on hand, allocated (set aside for jobs) and available. **Stock a product** picks a catalog product (a *product*, not a service or bundle) and how it's tracked:

- **By quantity:** just a count, for things like cables and screws.
- **By serial number:** each unit by its serial. Movements name the serials, and the count follows from them.
- **By lot:** batches. Movements name the lot, and each lot's stock is kept apart.

You can change how an item is tracked while none is on hand.

The item's page shows the stock at each location (with lots), the serial numbers in stock, and the ledger of movements.

## Moving stock

From the item's page (`inventory:write`):

- **Receive:** into a location, with what each one cost and a reference such as a delivery note or PO.
- **Transfer:** from one location to another. Stock that's allocated can't be moved.
- **Adjust:** a correction after a count, up or down, saying why. A serial-tracked unit can be added (found) or written off (lost).
- **Return from a job:** parts that come back into stock, such as a used serial unit.
- **Send back (RMA):** out to the vendor, with the RMA number.

The ledger keeps every movement, and none of them change afterwards.

## Parts on tickets and projects

Tickets and projects have a **Parts** card. **Add part** takes an item from a location in one of two ways:

- **Set aside** (allocated): the parts stay on hand but are no longer available. When the work is done, mark them **Used**, or **Put back** if they weren't needed.
- **Used already:** they leave stock straight away.

Used parts record the last cost paid, so job costing and billing can pick them up. A serial number's page entry shows which ticket it went to.

## Reorder points

Set a **reorder point** for an item at a location: when what's available there falls to it, Huxley raises `stock.low`. This happens once per dip; it clears when stock is back above the point. An item that's at or under a reorder point anywhere shows **Low** in the list. The reorder quantity is a suggestion for purchase orders.

## Events

For workflows:

- `inventory.received`, `inventory.transferred`, `inventory.adjusted`
- `inventory.allocated`, `inventory.released`, `inventory.consumed`
- `inventory.returned`, `inventory.rma`
- `stock.low`

For example, a workflow can tell purchasing when stock runs low, or bill used parts to the client's agreement.

## Permissions

| Permission | What it allows |
| --- | --- |
| `inventory:read` | See locations, items, stock, serial numbers and movements. Technicians, sales and read-only users have it. |
| `inventory:write` | Receive, move, adjust, return and send back stock, and take parts for tickets and projects. Technicians have it. |
| `inventory:configure` | Keep locations, which products are stocked, and reorder points. Administrators have it. |
