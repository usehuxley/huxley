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
| `crates/huxley-db` | PostgreSQL pools, org-scoped transactions (RLS), audit log, outbox, migrations |
| `crates/huxley-orgs` | Organization tree |
| `crates/huxley-auth` | Passwords, sessions, sign-in |
| `crates/huxley-authz` | Permissions, roles, scoped role assignments |
| `crates/huxley-modules` | Feature module registry and toggles |
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
just bootstrap # once: root organization + first user (prints a password)
just web       # staff app on :5173
just portal    # client portal on :5174
```

Sign in at http://localhost:5173 with `admin@example.com` and the printed password. End-to-end tests that need the API run when `E2E_EMAIL` and `E2E_PASSWORD` are set.

Grafana (traces, logs, metrics from your local server) is at http://localhost:3000, and Mailpit is at http://localhost:8025.

Run `just check` before pushing; it runs what CI runs. Other recipes: `just test-db` (database integration tests), `just e2e`, `just api-client` (regenerate the TS client after API changes), `just deny`. Run `just` for the full list.

## Running it

```sh
cd deploy/compose
./install.sh
docker compose up -d --build
docker compose run --rm huxley admin bootstrap \
  --org-name "Your MSP" --email you@example.com --name "Your Name" \
  --preset automation   # or service-desk, or full
```

`bootstrap` creates the root organization and the first user (with the Owner role), and prints a generated password once. Change it from your profile after signing in. If every owner is locked out, `huxley admin create-user --owner` creates a new owner.

Users add passkeys and turn on two-factor authentication (an authenticator app, with single-use recovery codes) from their profile. A passkey works both as the second factor after a password and on its own for passwordless sign-in. Teams (Settings → Teams) group people and service accounts; roles granted to a team apply to every member, on top of their own. For scripts and integrations, people create personal access tokens on their profile (scoped, expiring within a year), and administrators create service accounts under Settings → Service accounts: non-human identities that hold roles and use API tokens only. Send a token as `Authorization: Bearer hxp_…`. Under Settings → Single sign-on, administrators connect Microsoft Entra ID, Google, Okta, Authentik, Keycloak, or any OpenID Connect provider, restrict it to their email domains, optionally create accounts on first sign-in, and map group or app-role claims to roles (see ADR-019). Under Settings → Security, administrators can require two-factor authentication for everyone, for particular roles, or for particular organizations; people it covers who haven't set it up can only do that until they have. If someone loses both, an administrator can remove it under Settings → Users; only an owner can do that for another owner.

The preset chooses which feature modules start on: `automation` (none: Huxley as an automation engine only), `service-desk`, or `full`. Change them any time under Settings → Modules. Modules not yet released stay off until a release includes them.

Then open https://localhost (staff app) and https://portal.localhost (client portal). Set `HUXLEY_DOMAIN` and `HUXLEY_PORTAL_DOMAIN` in `.env` for real hostnames; Caddy obtains certificates automatically.

### Rotating the key-encryption key

Secrets are encrypted with per-provider-org data keys, which are in turn encrypted ("wrapped") by the KEK. To rotate the KEK: generate a new one, set it as `HUXLEY_CRYPTO__KEK` with the old one in `HUXLEY_CRYPTO__RETIRED_KEKS`, restart, run `huxley admin rewrap-keys`, then remove the retired key. The server refuses to start if any stored data key was wrapped by a KEK it doesn't have.

### Configuration

The server reads, in increasing precedence: built-in defaults, `huxley.toml`, then `HUXLEY_<SECTION>__<KEY>` environment variables. Any variable can instead be supplied as a file by appending `_FILE` (for Docker and Kubernetes secrets).

| Variable | Default | Purpose |
|---|---|---|
| `HUXLEY_DATABASE__URL` | (required) | PostgreSQL connection URL |
| `HUXLEY_CRYPTO__KEK` | (required) | Key-encryption key for secrets at rest: 32 random bytes, base64 (`huxley admin generate-kek`). Back it up: without it, encrypted data is unrecoverable. |
| `HUXLEY_AUTH__PUBLIC_URL` | (unset) | The staff app's URL as browsers see it, e.g. `https://huxley.example.com`. Enables passkeys, which are bound to its host name (changing the host later invalidates registered passkeys), and single sign-on, whose redirect URI is `<public URL>/login/oidc/callback`. |
| `HUXLEY_EMAIL__SMTP_URL` | (unset) | SMTP server for password reset, invitations, and verification, e.g. `smtps://user:pass@smtp.example.com` or `smtp://user:pass@host:587?tls=required`. Unset or empty: email is off. Links need `HUXLEY_AUTH__PUBLIC_URL`. |
| `HUXLEY_EMAIL__FROM` | (required with SMTP) | From address, e.g. `Huxley <huxley@example.com>` |
| `HUXLEY_CRYPTO__RETIRED_KEKS` | (empty) | Previous KEKs as a list, e.g. `[old1, old2]`, during rotation |
| `HUXLEY_HTTP__BIND` | `0.0.0.0:8080` | API listen address |
| `HUXLEY_MIGRATE_ON_START` | `false` | Apply migrations at startup |
| `HUXLEY_ROLES` | `all` | `api`, `worker`, `scheduler`, or `all` |
| `HUXLEY_TELEMETRY__OTLP_ENDPOINT` | (unset) | OTLP gRPC endpoint; unset disables export |
| `HUXLEY_TELEMETRY__LOG_FORMAT` | `json` | `json` or `pretty` |
| `HUXLEY_TELEMETRY__SAMPLE_RATIO` | `1.0` | Trace sampling ratio for new traces |
| `HUXLEY_HTTP__TRUST_FORWARDED_FOR` | `false` | Trust the rightmost `X-Forwarded-For` entry (enable behind your own proxy only) |
| `HUXLEY_HTTP__COOKIE_SECURE` | `true` | `Secure` flag on the session cookie; disable only for local HTTP |
| `HUXLEY_AUTH__SESSION_IDLE_MINUTES` | `480` | Sign out after this long without activity |
| `HUXLEY_AUTH__SESSION_ABSOLUTE_HOURS` | `168` | Sign out this long after sign-in regardless |
| `HUXLEY_AUTH__LOCKOUT_THRESHOLD` | `5` | Failed sign-ins before a temporary lockout |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Report security issues privately as described in [SECURITY.md](SECURITY.md).
