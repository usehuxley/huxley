# Assets

**Assets** (the `cmdb` module, part of the *Service desk* preset) keep what your clients have: workstations, servers, network devices, SaaS tenants, licenses, domains and certificates. They also record how those depend on each other.

## Assets and their types

Each asset belongs to a client (or to your MSP) and has:

- A **type**, which brings its own fields. A server has an operating system, role, memory and whether it's virtual; a domain has its registrar and expiry date; a network device has its role, firmware and management IP.
- A **status** through its life: planned, in stock, active, in maintenance, retired, disposed. Lists show assets in use (not retired or disposed) unless you pick a status.
- **Who uses it** (a contact of the client) and **where it is** (a client site).
- A serial number, hostname, manufacturer, model, purchase date and warranty end date, and notes.
- **External ids**, such as the device's id in an RMM, for matching when assets are synced.

Assets also take the custom fields, tags and attachments every record has (under **Settings → Fields**, for *Asset*).

Find them under **Assets**, filtered by client, type and status, or on a client's **Assets** tab. Search covers names, serial numbers, hostnames, models and manufacturers.

### Types

The built-in types are Workstation, Server, Network device, Printer, Mobile device, SaaS tenant, License, Domain, Certificate, Application and Other. With `cmdb:configure` you can add types and change any type's fields under **Settings → Asset types** (or through `/api/v1/asset-types`). Each field gets a stored key from its label when it's added; renaming the label later keeps the key, so existing values stay. Choices take one option a line, or `value | Label` to set the stored value. A field is text, long text, number, yes/no, date, a choice or several, a URL or an email, and may be required. Built-in types can be extended but not removed; your own types can be archived.

## Relationships and impact

Relate assets with typed relationships, read as *this asset … that one*:

| Relationship | Example |
|---|---|
| runs on | The ERP VM runs on the hypervisor |
| depends on | The ERP app depends on the SQL server |
| hosts | The hypervisor hosts the VM |
| is licensed by | The workstation is licensed by the Microsoft 365 license |
| backs up | The NAS backs up the file server |
| connects to | The switch connects to the firewall |

An asset's **If it goes down** card shows the impact: everything that runs on it, depends on it, is hosted by it or is licensed by it, then what depends on those, nearest first, and through which asset. Backups and network links don't carry impact. Retired and disposed assets are left out. Relationships connect assets of the same client, or of a client and your MSP.

### Dependency map

An asset with relationships shows its **dependency map**: the assets up to two relationships away, either way, and how they're related. What depends on something sits to its left, so the arrows point at what's depended on; a host is to the right of what it hosts. Click an asset to open it. A map shows at most 150 assets.

## Assets on tickets

With the service desk on, a ticket's **Assets** card shows what the ticket is about:

- **Linked assets.** Link one to keep it with the ticket; it must be the ticket's client's, or your MSP's.
- **The requester's:** the assets in use by the ticket's requester, ready to link.
- **Find an asset to link:** search assets from the ticket.

Linking needs `ticket:write` on the ticket and `asset:read` where the asset is. An asset's page lists its **Tickets**, newest first.

## Permissions

| Permission | Lets you |
|---|---|
| `asset:read` | See assets, their relationships and impact. |
| `asset:write` | Add, change, relate and delete assets. |
| `cmdb:configure` | Keep asset types and their fields (instance-wide). |

Technicians have `asset:read` and `asset:write`, and read-only users have `asset:read`.

## Events

`asset.created`, `asset.updated`, `asset.status_changed` (with `change.from` and `change.to`) and `asset.deleted` carry the asset's `name`, `type`, `status`, `serialNumber`, `hostname` and the `actorId`.

## API

| Endpoint | |
|---|---|
| `GET /api/v1/assets?q=…&orgId=…&typeId=…&status=…&inUse=true` | Assets you can see, with `total` |
| `POST /api/v1/assets`, `GET`, `PUT`, `DELETE /api/v1/assets/{id}` | Add (`orgId`, `typeId`, `name`, `attributes`, …), read with relationships, replace (send the `version` you read), delete |
| `POST /api/v1/assets/{id}/relationships`, `DELETE /api/v1/asset-relationships/{id}` | Relate (`kind`, `toId`), unrelate |
| `GET /api/v1/assets/{id}/impact?depth=…` | What breaks if it goes down |
| `GET /api/v1/assets/{id}/map?depth=…` | Its dependency map: `{ nodes, edges, truncated }` |
| `GET /api/v1/assets/{id}/tickets` | The tickets it's linked to |
| `GET`, `POST /api/v1/tickets/{id}/assets`, `DELETE …/assets/{assetId}` | A ticket's assets and the requester's; link (`assetId`), unlink |
| `GET`, `POST /api/v1/asset-types`, `PUT`, `DELETE /api/v1/asset-types/{id}` | Types and their fields |

## Coming next

Syncing assets from datasets (RMM, Microsoft 365) by matching rules, and warranty lookups.
