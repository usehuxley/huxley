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

## Warranties

An asset's **Warranties** card keeps its warranties: the manufacturer's, an extension, or a support contract. Each has a provider, a reference (contract number or service tag), start and end dates, and where it came from (entered by hand, or recorded by a vendor lookup). The asset's *Warranty ends on* follows its latest warranty, and **Assets** can be filtered to warranties ending within 30 or 90 days, already-ended ones included.

Each warranty is announced once when it's 60 days from ending, as `asset.warranty_expiring`, and once when it has ended, as `asset.warranty_expired`, so a workflow can open a renewal ticket or tell the account manager. Changing the end date re-arms both. Retired and disposed assets aren't announced. A vendor lookup workflow records what it finds with the `core.asset.set_warranty` step, which updates its earlier record for the same reference rather than adding another.

## Syncing from datasets

Assets can be kept in step with another system, such as an RMM's devices or Microsoft 365's tenants, through a [dataset](datasets.md) and a mapping rule under **Settings → Asset sync** (`cmdb:configure`). Each dataset row becomes an asset of the row's organization.

A rule has:

- **A source name**, such as `rmm`. It's where the asset's id in that system is kept (`externalIds.rmm`), and how its fields are marked.
- **Match keys, in order:** the id in the source, serial number, hostname or name, each read from a column. The first key that finds exactly one asset of the row's organization wins. If a key finds more than one, the row is skipped as ambiguous.
- **Fields to fill in:** columns for the name, hostname, serial number, manufacturer, model and id in the source, and the type's own fields. Empty values never clear anything.
- **What happens to values changed by hand:** *keep them*, so the source fills only what nobody changed, or *replace them* with the source's value.
- **Whether to create assets** of a type you choose for rows that match none. A new asset is named from its name, hostname or serial number column.

**Preview** shows what a sync would do without keeping anything. **Sync now** runs it. Both report rows created, updated, unchanged and skipped, with the first problems and why. To sync on a schedule, add a `core.asset.sync` step after the step that fills the dataset, in the same workflow.

Every asset field remembers where its value came from: `manual`, or the source. The asset page marks synced fields with their source, and lists the asset's ids in other systems and when it was last synced.

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

`asset.warranty_expiring` and `asset.warranty_expired` carry the asset's `name` and the warranty's `kind`, `provider`, `reference` and `endsOn`. `asset.sync_completed` follows each sync with its result. `asset.created`, `asset.updated` (with `change.source` and `change.fields` when a sync changed it), `asset.status_changed` (with `change.from` and `change.to`) and `asset.deleted` carry the asset's `name`, `type`, `status`, `serialNumber`, `hostname` and the `actorId`.

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
| `GET`, `POST /api/v1/asset-mappings`, `PUT`, `DELETE /api/v1/asset-mappings/{id}` | Dataset → asset mapping rules |
| `GET`, `POST /api/v1/assets/{id}/warranties`, `PUT`, `DELETE /api/v1/asset-warranties/{id}` | An asset's warranties |
| `POST /api/v1/asset-mappings/{id}/run?preview=true` | Run a rule now, or preview it: `{ rows, created, updated, unchanged, skipped, problems }` |

## Coming next

Integration packs for RMMs that fill the datasets, and vendor warranty lookups (Dell, Lenovo, HP) built on `core.asset.set_warranty`.
