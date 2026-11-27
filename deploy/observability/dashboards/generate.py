"""Generates Huxley's Grafana dashboards (ADR-091): `python3 generate.py` (3.12+).

Edit the panels here, not the JSON. Metric names are as the OpenTelemetry
Collector's Prometheus exporter writes them.
"""

import json
from pathlib import Path

DS = {"type": "prometheus", "uid": "${datasource}"}
J = '{job=~"$job"}'


def sel(extra=""):
    """A label selector with the job filter and `extra` matchers."""
    inner = 'job=~"$job"' + (", " + extra if extra else "")
    return "{" + inner + "}"


class Board:
    def __init__(self):
        self.panels = []
        self.y = 0
        self.x = 0
        self.row_h = 0
        self.next_id = 1

    def _place(self, w, h):
        if self.x + w > 24:
            self.y += self.row_h
            self.x = 0
            self.row_h = 0
        pos = {"x": self.x, "y": self.y, "w": w, "h": h}
        self.x += w
        self.row_h = max(self.row_h, h)
        return pos

    def row(self, title):
        if self.x:
            self.y += self.row_h
            self.x, self.row_h = 0, 0
        self.panels.append({"type": "row", "title": title, "id": self.next_id, "collapsed": False,
                            "gridPos": {"x": 0, "y": self.y, "w": 24, "h": 1}, "panels": []})
        self.next_id += 1
        self.y += 1

    def _panel(self, kind, title, targets, w, h, unit=None, description=None, extra=None):
        panel = {
            "type": kind,
            "title": title,
            "id": self.next_id,
            "datasource": DS,
            "gridPos": self._place(w, h),
            "targets": [
                {"refId": chr(65 + i), "datasource": DS, "expr": expr, "legendFormat": legend}
                for i, (expr, legend) in enumerate(targets)
            ],
            "fieldConfig": {"defaults": {}, "overrides": []},
            "options": {},
        }
        self.next_id += 1
        if unit:
            panel["fieldConfig"]["defaults"]["unit"] = unit
        if description:
            panel["description"] = description
        if extra:
            for k, v in extra.items():
                panel[k] = v
        self.panels.append(panel)
        return panel

    def stat(self, title, expr, unit=None, description=None, thresholds=None, w=4):
        p = self._panel("stat", title, [(expr, "")], w, 4, unit, description)
        p["options"] = {"reduceOptions": {"calcs": ["lastNotNull"], "fields": "", "values": False},
                        "colorMode": "value", "graphMode": "area", "textMode": "auto"}
        steps = [{"color": "green", "value": None}]
        for value, color in thresholds or []:
            steps.append({"color": color, "value": value})
        p["fieldConfig"]["defaults"]["thresholds"] = {"mode": "absolute", "steps": steps}
        return p

    def series(self, title, targets, unit=None, description=None, w=12, h=8, stack=False):
        p = self._panel("timeseries", title, targets, w, h, unit, description)
        custom = {"lineWidth": 1, "fillOpacity": 10, "showPoints": "never"}
        if stack:
            custom["stacking"] = {"mode": "normal", "group": "A"}
        p["fieldConfig"]["defaults"]["custom"] = custom
        p["options"] = {"legend": {"displayMode": "list", "placement": "bottom"},
                        "tooltip": {"mode": "multi", "sort": "desc"}}
        return p

    def table(self, title, expr, unit=None, description=None, w=12, h=8):
        p = self._panel("table", title, [(expr, "")], w, h, unit, description)
        p["targets"][0]["format"] = "table"
        p["targets"][0]["instant"] = True
        p["transformations"] = [{"id": "organize", "options": {"excludeByName": {"Time": True}}}]
        return p


def dashboard(uid, title, description, board):
    return {
        "uid": uid,
        "title": title,
        "description": description,
        "tags": ["huxley"],
        "timezone": "browser",
        "schemaVersion": 39,
        "version": 1,
        "editable": True,
        "graphTooltip": 1,
        "refresh": "30s",
        "time": {"from": "now-6h", "to": "now"},
        "templating": {"list": [
            {"type": "datasource", "name": "datasource", "label": "Data source", "query": "prometheus",
             "current": {}, "hide": 0},
            {"type": "query", "name": "job", "label": "Service", "datasource": DS,
             "query": {"query": "label_values(http_server_request_duration_seconds_count, job)",
                       "refId": "job"},
             "definition": "label_values(http_server_request_duration_seconds_count, job)",
             "includeAll": True, "multi": True, "allValue": ".*", "refresh": 2,
             "current": {"text": "All", "value": "$__all"}, "hide": 0},
        ]},
        "annotations": {"list": []},
        "links": [{"title": "Huxley dashboards", "type": "dashboards", "tags": ["huxley"], "asDropdown": True}],
        "panels": board.panels,
    }


def service():
    b = Board()
    req = "http_server_request_duration_seconds"
    b.row("API")
    b.stat("Requests / s", f"sum(rate({req}_count{J}[5m]))", "reqps")
    b.stat("Errors (5xx)", f'(sum(rate({req}_count{sel('http_response_status_code=~"5.."')}[5m])) or vector(0)) / sum(rate({req}_count{J}[5m]))',
           "percentunit", thresholds=[(0.01, "orange"), (0.05, "red")])
    b.stat("Latency p95", f"histogram_quantile(0.95, sum by (le) (rate({req}_bucket{J}[5m])))", "s",
           thresholds=[(1, "orange"), (2, "red")])
    b.stat("Sign-ins refused / min", f'(sum(rate(huxley_rate_limit_rejected_total{sel('scope="login"')}[5m])) or vector(0)) * 60',
           description="Sign-in attempts refused by the rate limit. Sustained, it may be password spraying.",
           thresholds=[(10, "orange"), (30, "red")])
    b.stat("API processes", f'count(count by (instance) ({req}_count{J}))')
    b.stat("DB connections in use", f'sum(db_client_connection_count{sel('db_client_connection_state="used"')})')
    b.series("Requests by status class", [
        (f'sum by (class) (label_replace(rate({req}_count{J}[5m]), "class", "${{1}}xx", "http_response_status_code", "([0-9]).."))', "{{class}}"),
    ], "reqps", stack=True)
    b.series("Latency", [
        (f"histogram_quantile(0.5, sum by (le) (rate({req}_bucket{J}[5m])))", "p50"),
        (f"histogram_quantile(0.95, sum by (le) (rate({req}_bucket{J}[5m])))", "p95"),
        (f"histogram_quantile(0.99, sum by (le) (rate({req}_bucket{J}[5m])))", "p99"),
    ], "s")
    b.table("Slowest routes (p95, last hour)",
            f"topk(15, histogram_quantile(0.95, sum by (le, http_request_method, http_route) (rate({req}_bucket{J}[1h]))))", "s")
    b.table("Busiest routes (requests / s, last hour)",
            f"topk(15, sum by (http_request_method, http_route) (rate({req}_count{J}[1h])))", "reqps")
    b.series("Errors by route", [
        (f'sum by (http_route, http_response_status_code) (rate({req}_count{sel('http_response_status_code=~"5.."')}[5m])) > 0',
         "{{http_response_status_code}} {{http_route}}"),
    ], "reqps", w=24)
    b.row("Database")
    b.series("Connections in use, by process", [
        (f'sum by (instance, db_client_connection_pool_name) (db_client_connection_count{sel('db_client_connection_state="used"')})',
         "{{instance}} {{db_client_connection_pool_name}}"),
    ], description="Against each pool's maximum (database.max_connections). Near the maximum, requests and jobs queue.")
    b.series("Pool use, by process", [
        (f'sum by (instance, db_client_connection_pool_name) (db_client_connection_count{sel('db_client_connection_state="used"')}) '
         f'/ sum by (instance, db_client_connection_pool_name) (db_client_connection_max{J})',
         "{{instance}} {{db_client_connection_pool_name}}"),
    ], "percentunit")
    b.row("Security")
    b.series("Requests refused by rate limits", [
        (f"sum by (scope) (rate(huxley_rate_limit_rejected_total{J}[5m])) * 60", "{{scope}}"),
    ], "short", description="Per minute, by limit: login, public_forms, oauth.", w=24)
    return dashboard("huxley-service", "Huxley — Service",
                     "API traffic, errors and latency; database connections; rate limits.", b)


def engine():
    b = Board()
    job = "huxley_job_duration_seconds"
    b.row("Queue")
    b.stat("Oldest waiting job", f"max(huxley_jobs_oldest_due_seconds{J})", "s",
           "How long the oldest job due to run has waited for a worker. Rising: too few workers, or they're stuck.",
           thresholds=[(60, "orange"), (300, "red")])
    b.stat("Jobs due", f'sum(max by (kind) (huxley_jobs{sel('state="due"')}))',
           thresholds=[(100, "orange"), (1000, "red")])
    b.stat("Jobs running", f'sum(max by (kind) (huxley_jobs{sel('state="running"')}))')
    b.stat("Dead-lettered (24h)", f"sum(increase(huxley_jobs_dead_lettered_total{J}[24h])) or vector(0)",
           description="Jobs given up on after repeated failures. Retry them from Settings → Workflow jobs.",
           thresholds=[(1, "orange")])
    b.stat("Runs finished / min", f"sum(rate(huxley_runs_finished_total{J}[5m])) * 60")
    b.stat("Runs failing", f'(sum(rate(huxley_runs_finished_total{sel('status="failed"')}[15m])) or vector(0)) / sum(rate(huxley_runs_finished_total{J}[15m]))',
           "percentunit", thresholds=[(0.1, "orange"), (0.25, "red")])
    b.series("Jobs by kind and state", [
        (f'max by (kind, state) (huxley_jobs{sel('state=~"due|running"')})', "{{kind}} {{state}}"),
    ], description="Due: waiting for a worker. Running: claimed by one. Each worker pool's KEDA trigger counts its kinds' due and running jobs.")
    b.series("Oldest waiting job", [(f"max(huxley_jobs_oldest_due_seconds{J})", "oldest due")], "s")
    b.series("Jobs done / s, by kind", [
        (f"sum by (kind, outcome) (rate({job}_count{J}[5m]))", "{{kind}} {{outcome}}"),
    ], "ops", stack=True)
    b.series("Job duration p95, by kind", [
        (f"histogram_quantile(0.95, sum by (le, kind) (rate({job}_bucket{J}[5m])))", "{{kind}}"),
    ], "s", description="Steps include calls to the outside world; advance and deadline jobs are bookkeeping.")
    b.series("Runs finished / min, by status", [
        (f"sum by (status) (rate(huxley_runs_finished_total{J}[5m])) * 60", "{{status}}"),
    ], stack=True)
    b.series("Workers busy, by process", [
        (f"sum by (instance) (rate({job}_sum{J}[5m]))", "{{instance}}"),
    ], description="Seconds of job work per second in each worker process: compare with its concurrency (engine.concurrency).")
    b.row("Audit log")
    b.series("Unsealed audit records", [(f"max(huxley_audit_unsealed{J})", "waiting to be sealed")],
             description="Sealed into the hash chain about a second after commit. A count that stays up means the scheduler's sealer has stopped.")
    b.series("SIEM streams: records behind", [(f"max by (stream) (huxley_audit_stream_behind{J})", "{{stream}}")],
             description="Failed attempts in a row show on Settings → Audit streaming.")
    return dashboard("huxley-engine", "Huxley — Workflow engine",
                     "The job queue, worker throughput, run outcomes, and the audit log's sealing and SIEM streams.", b)


if __name__ == "__main__":
    here = Path(__file__).parent
    for name, board in [("huxley-service.json", service()), ("huxley-engine.json", engine())]:
        (here / name).write_text(json.dumps(board, indent=2, ensure_ascii=False) + "\n")
        print("wrote", name)
