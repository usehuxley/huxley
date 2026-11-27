# ADR-091: Application metrics over OpenTelemetry, with Grafana dashboards and tested alert rules

- **Status:** Accepted (implementer's design). Implements production dashboards and alert rules from Phase 13 (plan §3).
- **Date:** 2026-11-27

## Context

Huxley already exported OpenTelemetry traces, logs and a meter provider, but recorded no metrics of its own. Operators had nothing to chart or alert on.

## Decision

**Instrumentation.** Each crate records instruments on OpenTelemetry's global meter. They're no-ops when export is off.
- **API:** the `http.server.request.duration` histogram. It's labeled with the matched route template from axum's `MatchedPath`, so ids never create series. It's added as a `route_layer` on the API, portal API and SCIM routers.
- **Rate limits:** a rejection counter.
- **Engine:** job duration by kind and outcome, dead-lettered jobs, and finished runs by status.
- **Database:** each process reports `db.client.connection.count` and `.max` per pool. Pools grow lazily, so the alert divides by the maximum, not the pool's current size.
- **Shared state:** queue state by kind, the oldest due job's wait, unsealed audit records, and SIEM stream lag. The scheduler reads these every 15 seconds and reports them as gauges, so other processes don't count them again. Every kind and state reports, as zero when empty, so series exist for alerts.
- **Instance label:** the resource gains `service.instance.id` (host name and pid), which collectors turn into Prometheus' `instance`. Without it, several pods' series would collide.

**What ships in `deploy/observability`:**
- the collector configuration, now with a Prometheus endpoint on `:8889` as well as tail sampling;
- two Grafana dashboards, generated from `generate.py`;
- twelve Prometheus alert rules, with `promtool` unit tests.

CI checks the rules (`check` and `test`) and fails if the dashboard JSON drifts from its generator.

**Verification.** Every dashboard query was run against a real Prometheus. It scraped a real collector fed by Huxley during e2e runs. All 30 queries parsed; the only empty ones were for events that hadn't happened (5xx responses, refusals, dead jobs), and those stats default to zero.

## Consequences

- Names assume the collector's (or Prometheus') default OTLP-to-Prometheus translation. Other backends see the OpenTelemetry names, such as `huxley.job.duration`, and need their own dashboards.
- The scheduler's gauge reads are four small queries every 15 seconds.
- Route labels bound the series count: a few hundred routes, times method, times status.
- The chart doesn't install dashboards or rules yet. The docs say how to import or wrap them.
