# Observability

Huxley sends traces, metrics and logs over OpenTelemetry (OTLP) when `HUXLEY_TELEMETRY__OTLP_ENDPOINT` is set. `deploy/observability` holds what's needed to watch it:

- `otel-collector.yaml`: an OpenTelemetry Collector configuration. It tail-samples traces, keeping every error and every slow request, and serves metrics for Prometheus on `:8889`.
- `dashboards/`: two Grafana dashboards, **Huxley — Service** and **Huxley — Workflow engine**.
- `alerts/huxley.rules.yaml`: Prometheus alert rules, with unit tests.

With Compose, `docker compose --profile observability up -d` starts the collector. Set `HUXLEY_OTLP_ENDPOINT=http://otel-collector:4317` in `.env`. With Helm, set `telemetry.otlpEndpoint` to your collector.

## Metrics

Names follow the OpenTelemetry semantic conventions where there is one. Prometheus sees dots as underscores, with units and `_total` appended; the names below are those. Every process is its own `instance` (its pod or host name), and `job` is the service name (`huxley`).

| Metric | Labels | What it is |
|---|---|---|
| `http_server_request_duration_seconds` (histogram) | `http_route`, `http_request_method`, `http_response_status_code` | API, portal API and SCIM requests. The route is the template (`/api/v1/tickets/{id}`), never the raw path. |
| `huxley_rate_limit_rejected_total` | `scope` (`login`, `public_forms`, `oauth`) | Requests refused by a rate limit. |
| `db_client_connection_count` | `db_client_connection_pool_name` (`main`, `listen`), `db_client_connection_state` (`used`, `idle`) | Each process's database connections. |
| `db_client_connection_max` | `db_client_connection_pool_name` | The most each pool may open (`database.max_connections`). |
| `huxley_job_duration_seconds` (histogram) | `kind` (`step`, `advance`, `deadline`), `outcome` (`ok`, `error`) | Workflow jobs, as workers ran them. |
| `huxley_jobs_dead_lettered_total` | `kind` | Jobs given up on after repeated failures. |
| `huxley_runs_finished_total` | `status` (`succeeded`, `failed`, `cancelled`) | Finished workflow runs. |
| `huxley_jobs` | `kind`, `state` (`due`, `running`, `scheduled`, `dead`) | The job queue, read every 15 seconds by the scheduler. |
| `huxley_jobs_oldest_due_seconds` | | How long the oldest job due to run has waited for a worker. This is the clearest sign workers are short. |
| `huxley_audit_unsealed` | | Audit records committed but not yet sealed into the hash chain (ADR-086). |
| `huxley_audit_stream_behind`, `huxley_audit_stream_failures` | `stream` | Each SIEM stream's backlog, and its failed attempts in a row (ADR-087). |

The scheduler reports the queue and audit gauges. With two schedulers, the dashboards take the larger value.

## Dashboards

Import the JSON files in `deploy/observability/dashboards` into Grafana (Dashboards → New → Import), or provision them. Each asks for a Prometheus data source, and can filter by service.

- **Service:** requests, errors and latency, the slowest and busiest routes, errors by route, database pool use by process, and rate limit refusals.
- **Workflow engine:**
  - the oldest waiting job, and jobs by kind and state;
  - throughput and p95 duration by kind;
  - runs by status;
  - how busy each worker process is;
  - unsealed audit records, and SIEM stream backlogs.

The JSON is generated: change `generate.py` and run it (Python 3.12+). CI checks the two match.

## Alerts

| Alert | Fires when |
|---|---|
| `HuxleyApiErrors` (critical) | More than 5% of requests answer 5xx for 10 minutes. |
| `HuxleyApiSlow` | p95 latency is over 2 seconds for 15 minutes. |
| `HuxleyJobsWaiting` (critical) / `HuxleyJobsDelayed` | A due job has waited more than 5 minutes (or 1 minute, for 10 minutes). Workers are down, stuck or too few. |
| `HuxleyJobsDeadLettered` | Any job is given up on. Retry it from Settings → Workflow jobs. |
| `HuxleyRunsFailing` | More than a quarter of runs fail for 15 minutes. |
| `HuxleySchedulerSilent` (critical) | No queue metrics for 10 minutes. Timers, schedules, retention, sealing and streaming have stopped, or telemetry has. |
| `HuxleyAuditNotSealing` (critical) | More than 100 records stay unsealed for 10 minutes. |
| `HuxleyAuditStreamFailing` / `HuxleyAuditStreamBehind` | A SIEM stream fails 5 times in a row, or falls 10,000 records behind for 30 minutes. |
| `HuxleyDatabasePoolSaturated` | A process uses over 90% of a pool's maximum for 10 minutes. |
| `HuxleySignInsRateLimited` | More than 30 sign-ins a minute are refused for 10 minutes. That's possibly password spraying. |

Load `huxley.rules.yaml` into Prometheus (`rule_files`), into Mimir or Grafana alerting, or wrap its groups in a `PrometheusRule` for the Prometheus Operator. `promtool test rules huxley.rules.test.yaml` runs its tests, as CI does.
