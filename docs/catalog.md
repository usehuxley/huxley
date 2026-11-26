# Product catalog

The **Catalog** (the `contracts` module) holds what you sell: products, services, and bundles of them, with costs and prices. [Agreements](agreements.md) bill them as recurring lines, and quotes and invoices will build on it. Turn the module on under **Settings → Modules**.

## Products, services and bundles

Each item has:

- **A SKU**, unique within its organization (in any case): letters, digits, dots, dashes, slashes or underscores.
- **A name and description**, an optional **category**, **vendor** and **manufacturer part number**.
- **A unit**: what one of it is, such as `each`, `hour`, `month`, `user` or `device`.
- **Cost and price**, in a **currency** (three letters, default `USD`).
- **Billing**: one time, monthly, quarterly or annual.
- Whether it's **taxable**, and whether it's **offered**.

A **bundle** lists other products and services of its organization (not other bundles), each with a quantity. Give it its own price, or leave the price empty and it shows what its components add up to.

**Retiring and deleting:** untick **Offered** to retire an item. Retired items are hidden unless you tick **Show retired**, but they stay on whatever already uses them. Only an item that no bundle contains can be deleted.

## Money

Amounts are kept exactly, as decimals with up to four places, each with its currency (design doc §8.11). The API takes amounts as strings (`"1149.00"`) or numbers. It answers with strings without trailing zeros (`"1149"`, `"22.5"`), which the app formats for display.

## Permissions

- `catalog:read` at an organization: see its catalog. Technicians and read-only users have it.
- `catalog:manage`: add, change, retire and delete items.

Changes are audited (`catalog_product.created`, `.updated` with cost, price and offered changes, `.deleted`).

## API

`/api/v1/catalog/products`:

- `GET` lists items, filtered by `q` (words in the name, or the start of the SKU), `kind`, `category`, `active` and `orgId`.
- `POST` adds one: `{ "orgId", "product": { … } }`.
- `GET`, `PUT` (with the `version` you read) and `DELETE` work on `/{id}`.
