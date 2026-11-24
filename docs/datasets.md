# Datasets

A **dataset** is a typed table of data kept in Huxley, such as "Microsoft 365 users for every client", so workflows, forms and reports can use it without calling an API each time (design doc §9, ADR-029). This page covers definitions, rows, the Datasets page, workflow steps, form choices, syncing from integrations, and the API. A read-only SQL console comes later.

## Definitions

A dataset has:

- a **key** (lowercase letters, digits and hyphens, fixed once created) and a name
- **columns**, each with a name (lowercase letters, digits and underscores; names starting with `_` are reserved) and a type: `text`, `integer`, `number`, `boolean`, `timestamp`, `date`, `json` or `uuid`
- an optional **primary key**: the columns that identify a row within an organization. Writing a row whose key exists updates it.

Changing a definition **migrates its table**:

- New columns are added and removed ones dropped.
- A changed type is converted. If any value doesn't convert (say, `"many"` to `integer`), the change is refused and nothing changes.

## Rows and organizations

Every row belongs to an organization: the dataset's own, or one below it. A dataset defined at your MSP can hold every client's rows. People see the rows of organizations in their scope, so a client's users see only that client's rows. This is enforced by the database's row-level security, as well as by the API.

## In the app

**Datasets** in the sidebar (with `dataset:read`) lists them. With `dataset:manage` you can also create one with its columns and key. A dataset's page has:

- its rows, filtered by a column's value or an organization, a page at a time
- its sync settings, with when it next and last synced, and **Sync now**
- a box for adding rows by hand

## In forms

A form field's choices can come from a dataset's rows: those of the organization the form is filled in for, and those below it.

```json
"ui": { "who": { "options_from": {
    "dataset": "m365-users", "filters": { "licensed": true }, "value": "upn", "label": "name" } } }
```

`value` (required) and `label` are columns. Submitted answers are checked against the choices, as with action lookups.

## In workflows

```yaml
- id: users
  type: dataset.query
  with: { dataset: m365-users, filters: { licensed: true }, order_by: upn }
- id: save
  type: dataset.upsert
  with: { dataset: m365-users, rows: "{{ steps.fetch.output.body.value }}" }
```

A run reads and writes the rows of its own organization and those below it:

- A workflow at your MSP can read every client's rows, and write rows for any client with `org_id`.
- A client's workflow sees only that client's rows.

`dataset.upsert` refuses rows containing secrets. See [the step reference](workflows.md).

## Syncing from an integration

A dataset can keep itself up to date from an integration's list action:

```json
PUT /api/v1/datasets/{id}/sync
{ "sync": {
    "action": "msgraph/listUsers",
    "map": { "upn": "userPrincipalName", "name": "displayName", "department": "department" },
    "mode": "replace",
    "orgs": "clients",
    "schedule": { "cron": "0 2 * * *" }
} }
```

How a sync works:

- **Each sync is a workflow run** in each target organization, visible under Runs with trigger `dataset_sync`. The run calls the action, following every page (the action's pagination is set in the integration's overlay). Then it writes the items to the dataset, picking each column by its path in `map`. It uses the integration's rate limits and each organization's connection and variables: for `orgs: clients`, every client is fetched with its own tenant.
- **`mode`:**
  - `replace` (the default) replaces each organization's rows with what was fetched. If there were more pages than `maxPages` (default 20, at most 100), it fails and replaces nothing, rather than deleting what it didn't fetch.
  - `upsert` adds and updates rows by the dataset's key, and never removes any.
- **`orgs`:** `own` (the default), `clients` (every client below the dataset's organization) or `all` (the organization and everything below it).
- **`schedule`:** `cron` (with an optional `timezone`; the default is the organization's) or `interval`. Without one, it syncs only when you start it with `POST /api/v1/datasets/{id}/sync/run`.
- **Permissions:** setting a sync needs `dataset:manage` and `connection:read`. `{ "sync": null }` stops it.

## The API

| | |
|---|---|
| `GET /api/v1/datasets` | the datasets you can see |
| `POST /api/v1/datasets` | `{ orgId, dataset: { key, name, columns, primaryKey } }` |
| `GET`, `PUT`, `DELETE /api/v1/datasets/{id}` | read, change (migrating the table) or delete it and its rows |
| `POST /api/v1/datasets/{id}/rows` | `{ orgId, rows: [ … ], replace }`: add rows (updating those whose key exists), or with `replace: true` replace all of that organization's rows |
| `POST /api/v1/datasets/{id}/rows/query` | `{ filters: { column: value }, orgId, orderBy: "-column", limit, offset }`: rows and the total that match |
| `POST /api/v1/datasets/{id}/rows/delete` | `{ filters, orgId }`: delete matching rows |
| `GET`, `PUT /api/v1/datasets/{id}/sync` | its sync settings, next and last run; set or stop them |
| `POST /api/v1/datasets/{id}/sync/run` | sync now: one run per target organization |

Values are checked against their column types. A write holds at most 10,000 rows, and a query returns at most 1,000. Reading needs `dataset:read` and changing needs `dataset:manage`, at the dataset's organization; Technician and Read-only have `dataset:read`. Creating, changing and deleting datasets is audited.

## How it's stored

Each dataset's rows live in their own table, `datasets.ds_<id>`, with the dataset's columns plus `_org_id` and `_synced_at`. The table is created and migrated by the database from the stored definition, which it checks again itself. The app never sends table definitions as SQL.
