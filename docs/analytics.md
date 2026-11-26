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

### Datasets

Every [dataset](datasets.md) is a source too: `dataset:<key>`, read with `dataset:read`.

- **Fields:** one per column (JSON columns aside), plus `_organization` (whose row it is) and `_synced_at` (when it was written).
- **Time:** the dataset's first date or timestamp column is its time field, else `_synced_at`.
- **Rows:** only those of the organizations where you can read datasets.
- **Which dataset:** those defined at an organization you can read datasets at, or above it. The same key at several organizations is the one nearest the top.

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

## Dashboards

**Dashboards** in the staff app show widgets built on these sources: a number, a table, bars, a line, an area, or a donut.

- **Prebuilt:**
  - **Service desk overview**
  - **SLA performance**
  - **Technician utilization**
  - **Client overview**
  - **Time and utilization**

  They're read-only: **Copy to change it** makes a personal copy.
- **Shared** dashboards live at an organization. People with `dashboard:read` there see them (or only the teams and roles it's published to), and `dashboard:manage` changes them. **Personal** dashboards are yours alone; `dashboard:read` is enough to keep them.
- **Range and organization:** pick a range (it's applied to each widget's source time field, unless the widget ignores the dashboard's range) and an organization (that client and those below it). Everyone sees the numbers their own access allows, so the same dashboard can show different numbers to different people.
- **Building a widget:** pick the data, a measure ("how many", "total of", "average of"…), a grouping (with a time bucket for dates), an optional split, and conditions. Charts can switch to a table of the same numbers.
- **The client portal:** a shared dashboard can be shown there, to client admins with their company's numbers. See [Client portal: Reports](portal.md#reports).
- **API:** `GET`/`POST /api/v1/dashboards`; `GET`/`PUT`/`DELETE /api/v1/dashboards/{id}` (a uuid, or a prebuilt dashboard's key such as `service-desk`); `POST /api/v1/dashboards/{id}/copy` `{ orgId, name? }`. Widgets store query specs, never SQL.

### Publishing

Anyone with `dashboard:read` can build a personal dashboard and ask for it to be shared (ADR-045):

1. **Submit:** on your dashboard, **Publish** asks who should see it: teams, roles (neither: everyone who sees dashboards there), and the client portal (its widgets must use tickets, time or assets). Add a note for the reviewer. What's submitted is the dashboard as it is then; later changes aren't part of it.
2. **Review:** people with `dashboard:manage` there find it under **Waiting for your approval** on the Dashboards page. The review page shows it as submitted, with their own numbers. Someone else has to decide your own submission.
3. **Decide:**
   - **Approve and publish** shares it, as submitted, with that audience (and the portal), as publication 1, 2, ….
   - **Send back** needs a comment; it stays personal and you see the comment on it. Change it and submit again.

You can withdraw a pending submission. Events: `dashboard.submitted`, `dashboard.published` and `dashboard.rejected` (with `name`, `submitted_by`, `comment`, `number`). The built-in **Dashboard publishing** pack tells administrators about submissions and authors about decisions.

API:
- `POST /api/v1/dashboards/{id}/submit` `{ teams, roles, portal, note }`
- `GET /api/v1/dashboards/{id}/submissions` (history)
- `GET /api/v1/dashboard-submissions` (pending, for you to decide)
- `GET /api/v1/dashboard-submissions/{id}`
- `POST /api/v1/dashboard-submissions/{id}/approve|reject` `{ comment }`, and `/withdraw`
- `GET /api/v1/dashboard-audiences`: the teams and roles to pick from
