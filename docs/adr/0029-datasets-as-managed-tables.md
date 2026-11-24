# ADR-029: Datasets are managed tables whose DDL the database generates

- **Status:** Accepted (implementer's design, per the maintainer's "you choose")
- **Date:** 2026-11-24

## Context

Datasets (design doc §9) keep data from integrations in Huxley as typed, indexed tables: "M365 users for every client". Their tables are created and changed at runtime, but the app connects as `huxley_app`, which can't run DDL. And generated DDL is a classic injection risk.

## Decision

- **Definitions:** a definition lives in `datasets` (public schema, row-level security by organization). It holds:
  - key and name
  - columns, each a name and a type from a fixed set: `text`, `integer`, `number`, `boolean`, `timestamp`, `date`, `json`, `uuid`
  - an optional primary key (column names)
- **Tables:** each dataset's rows live in `datasets.ds_<id without hyphens>`. The table name comes from the id, never from user input. Besides the dataset's own columns, each table has:
  - `_org_id`: the organization a row belongs to
  - `_synced_at`
  - with a primary key, a unique index on `(_org_id, key columns)`, so upserts match within an organization
- **The database writes the DDL.** `huxley_dataset_apply(id)` is a `SECURITY DEFINER` function owned by the migration user. It reads the stored definition and checks every name and type again in SQL. Then it creates the table, or brings it up to date:
  - adds and drops columns
  - changes types with a cast; if values don't convert, it fails and nothing changes
  - rebuilds the key index

  Identifiers go through `format('%I')`. `huxley_dataset_drop(id)` removes the table. The app never sends DDL text.
- **Row security:** every dataset table has row-level security with the usual `huxley_org_in_scope(_org_id)` policy. People see rows of organizations in their scope, so a dataset defined at a provider holds every client's rows, and each client sees only its own.
- **Row access from the app:**
  - **Writes** go through `jsonb_populate_recordset(NULL::table, $1)`, so values are typed by the table itself.
  - **Filters** compare a column with a bound parameter cast to the column's type from the fixed set.
  - Column names are checked against the definition before they reach SQL.

## Consequences

- Dataset tables are ordinary Postgres tables: they're fast, indexable, and queryable read-only by admins (the SQL console, later).
- A type change that loses data fails rather than truncating.
- Columns starting with `_` are reserved.
