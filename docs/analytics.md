# Reporting data

The semantic data layer (ADR-041) answers questions like "open tickets by client" or "billable hours by technician this month" without SQL. Dashboards and widgets are built on it, and so is the API below.

## Sources

Each source is a curated view of one kind of record. You can query a source if you hold its permission somewhere and its module is on there. The numbers cover only those organizations.

| Source | What it is | Module | Permission | Fields |
|---|---|---|---|---|
| `tickets` | Service desk tickets, with their status, queue, assignee and SLA outcome. | servicedesk | `ticket:read` | `client` (text), `status` (text), `status_category` (text), `open` (bool), `priority` (text), `type` (text), `queue` (text), `assignee` (text), `team` (text), `source` (text), `created_at` (time), `first_response_at` (time), `resolved_at` (time), `closed_at` (time), `response_minutes` (number), `resolution_hours` (number), `response_met` (bool), `resolution_met` (bool) |
| `time_entries` | Time logged on tickets: who, for which client, what kind of work, and whether it's billable. | servicedesk | `ticket:read` | `client` (text), `technician` (text), `work_type` (text), `worked_on` (date), `hours` (number), `minutes` (number), `billable` (bool), `ticket` (text) |
| `assets` | Configuration items: devices and other assets, with their type, status and warranty. | cmdb | `asset:read` | `client` (text), `type` (text), `status` (text), `manufacturer` (text), `model` (text), `site` (text), `purchased_on` (date), `warranty_ends_on` (date), `in_warranty` (bool), `created_at` (time) |
| `runs` | Workflow runs: which workflow, how it was triggered, how it ended and how long it took. | (none) | `workflow:read` | `organization` (text), `workflow` (text), `status` (text), `trigger` (text), `created_at` (time), `finished_at` (time), `duration_seconds` (number) |

## Asking a question

`POST /api/v1/analytics/query` with a query spec:

```json
{
  "source": "time_entries",
  "measures": [{ "fn": "sum", "field": "hours" }, { "fn": "count" }],
  "dimensions": [{ "field": "technician" }, { "field": "worked_on", "granularity": "week" }],
  "filters": [
    { "field": "billable", "op": "eq", "value": true },
    { "field": "worked_on", "op": "within", "value": "this_month" }
  ],
  "sort": [{ "by": "sum_hours", "dir": "desc" }],
  "limit": 50,
  "timezone": "Europe/Berlin"
}
```

- **Measures:**
  - `count`
  - `count_distinct` (any field)
  - `sum` and `avg` (number fields)
  - `min` and `max` (number, date or time fields)
- **Measure keys:** `count`, `count_distinct_<field>`, `sum_<field>`, `avg_<field>`, `min_<field>`, `max_<field>`.
- **Dimensions:** group by fields. Date and time fields can be bucketed by `hour` (times only), `day`, `week`, `month`, `quarter` or `year`, in the spec's `timezone` (default UTC).
- **Filters:**
  - `eq`, `neq`, `in`, `not_in` (a list)
  - `gt`, `gte`, `lt`, `lte`, `between` (`[from, to]`), for numbers, dates and times
  - `contains` (text, any case)
  - `is_null`, `not_null`
  - `within` a named range: `today`, `yesterday`, `last_7_days`, `last_30_days`, `last_90_days`, `this_week`, `last_week`, `this_month`, `last_month`, `this_quarter`, `last_quarter`, `this_year`, `last_year`, `last_12_months`
- **`orgId`** narrows to one organization and those below it, e.g. one client.
- **Sort** by any column's key. By default, a time series is sorted by time, and anything else biggest first.

The answer has `columns` (key, label, kind) and `rows` (arrays in column order). `truncated` says more rows matched than the limit, and `cached` says the answer came from the last minute's results.

`GET /api/v1/analytics/sources` lists the sources you can query, with their fields and the named ranges.

## Limits

- At most 3 dimensions, 6 measures, 20 filters and 1,000 rows (default 100).
- A query gets 5 seconds. A slower one fails with **That took too long**: narrow the time range, group by less, or add a filter.
- Answers are reused for a minute, per access level and spec.
- Set `HUXLEY_DATABASE__READ_URL` to send these queries to a read replica.
