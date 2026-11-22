# Huxley

Huxley is a free, open-source automation platform for MSPs, MSSPs, and service desks. At its core is a workflow engine: build automations on a visual canvas, as code, or through AI over MCP, and connect any system by importing its OpenAPI spec. On top of the engine sit the modules service providers run on: ticketing, assets, knowledge, contracts, projects, sales and quoting, reporting, invoicing, and a client portal.

Every module runs on the engine. Mailbox polling, SLA escalation, AI triage, and technician actions are all workflows you can open, copy, and change.

> **Status:** early development (Phase 0: foundations). Not ready for production use.
> See the [implementation plan](docs/IMPLEMENTATION_PLAN.md) for the roadmap.

- Website: [usehuxley.com](https://usehuxley.com)
- Docs: [docs.usehuxley.com](https://docs.usehuxley.com)
- License: [MIT](LICENSE)

## Repository layout

| Path | What it is |
|---|---|
| `crates/huxley-server` | The `huxley` binary: API, worker, and scheduler roles |
| `crates/huxley-api` | HTTP API, conventions, OpenAPI document |
| `crates/huxley-db` | PostgreSQL pool, org-scoped transactions (RLS), migrations |
| `crates/huxley-telemetry` | OpenTelemetry setup (traces, metrics, logs via OTLP) |
| `crates/huxley-core` | Shared types: ids, pagination |
| `migrations/` | SQL migrations, embedded in the binary |
| `apps/web` | Staff web app (SvelteKit) |
| `apps/portal` | Client portal (SvelteKit) |
| `packages/ui` | Design system shared by both apps |
| `packages/api-client` | TypeScript API client generated from the OpenAPI document |
| `docker/` | Container images |
| `deploy/compose` | Single-host reference deployment |
| `deploy/observability` | OpenTelemetry Collector config |
| `docs/` | Design document, plan, ADRs, conventions |

## Development

Prerequisites: Rust 1.99 (installed automatically from `rust-toolchain.toml`), Node 24+, pnpm 12, Docker, and [just](https://just.systems).

```sh
just setup     # pnpm install + Playwright browser
just dev-up    # Postgres, Valkey, Mailpit, OpenTelemetry + Grafana

just server    # API on :8080 (applies migrations on start)
just web       # staff app on :5173
just portal    # client portal on :5174
```

Grafana (traces, logs, metrics from your local server) is at http://localhost:3000, and Mailpit is at http://localhost:8025.

Run `just check` before pushing; it runs what CI runs. Other recipes: `just test-db` (database integration tests), `just e2e`, `just api-client` (regenerate the TS client after API changes), `just deny`. Run `just` for the full list.

## Running it

```sh
cd deploy/compose
./install.sh
docker compose up -d --build
```

Then open https://localhost (staff app) and https://portal.localhost (client portal). Set `HUXLEY_DOMAIN` and `HUXLEY_PORTAL_DOMAIN` in `.env` for real hostnames; Caddy obtains certificates automatically.

### Configuration

The server reads, in increasing precedence: built-in defaults, `huxley.toml`, then `HUXLEY_<SECTION>__<KEY>` environment variables. Any variable can instead be supplied as a file by appending `_FILE` (for Docker and Kubernetes secrets).

| Variable | Default | Purpose |
|---|---|---|
| `HUXLEY_DATABASE__URL` | (required) | PostgreSQL connection URL |
| `HUXLEY_HTTP__BIND` | `0.0.0.0:8080` | API listen address |
| `HUXLEY_MIGRATE_ON_START` | `false` | Apply migrations at startup |
| `HUXLEY_ROLES` | `all` | `api`, `worker`, `scheduler`, or `all` |
| `HUXLEY_TELEMETRY__OTLP_ENDPOINT` | (unset) | OTLP gRPC endpoint; unset disables export |
| `HUXLEY_TELEMETRY__LOG_FORMAT` | `json` | `json` or `pretty` |
| `HUXLEY_TELEMETRY__SAMPLE_RATIO` | `1.0` | Trace sampling ratio for new traces |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Report security issues privately as described in [SECURITY.md](SECURITY.md).
