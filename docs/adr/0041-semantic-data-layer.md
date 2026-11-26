# ADR-041: A semantic data layer: a catalog of sources and a query spec compiled to SQL

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Dashboards, widgets and reports (design doc §8.12) need numbers from across the product: tickets, time, assets, workflow runs and, later, datasets. Users shouldn't write SQL, and whatever they build must respect what they may see. The same widgets appear in the staff app and, once published, in the client portal.

## Decision

**Sources are defined in code.** A source is a curated view of one kind of record. It defines:

- its `FROM` (a base table with the joins it needs, always including the owning organization as `o`)
- the module it belongs to, and the permission that reads it
- its fields: a key, a label, a kind (`text`, `number`, `bool`, `date`, `time`) and a SQL expression

The first sources are `tickets`, `time_entries`, `assets` and `runs`. Datasets (§9) follow as sources whose fields come from their columns.

**Queries are structured, never SQL.** A query spec names:

- a source
- measures: `count`, `count_distinct`, `sum`, `avg`, `min`, `max`
- dimensions: fields, with a time bucket for dates and times
- filters, sort, a limit, an optional organization (that client and the ones below), and a time zone

The compiler:

- **Identifiers:** looks every field up in the catalog. Identifiers in the SQL only ever come from the catalog.
- **Values:** binds every value as a parameter.
- **Shape:** checks that each operator and measure suits the field's kind.

**Who sees what.** The caller's scope is computed for the source's permission and module, the same way list pages scope their queries. The compiled SQL filters the source's organization column to those organizations. The query runs in a read-only transaction. A user who can read tickets at two clients gets numbers for those two, and no one else's.

**Cost limits.**

- At most 3 dimensions, 6 measures, 20 filters and 1,000 rows.
- A 5-second statement timeout. A query that exceeds it fails with a clear message, not a hung page.

**Caching.** Results are cached in memory for 60 seconds. The key is the scope and the spec, so two people with the same access share an entry and nobody sees another scope's numbers.

**Read replica.** When `database.read_url` is set, queries go to that pool. Otherwise they use the primary. The scope filter is in the SQL, so the replica needs nothing extra.

**Output.** Rows are arrays in column order, alongside column descriptions (key, label, kind). Numbers come out as JSON numbers, times as RFC 3339, and dates as `YYYY-MM-DD`.

## Consequences

- Adding a source is a code change and a review, which is deliberate: each source is a security and performance decision.
- Widgets, dashboards and portal publishing build on this API. They store query specs, never SQL.
- The catalog is the place to add measures with business meaning later (e.g. "billable %").
