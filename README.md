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

Users add passkeys and turn on two-factor authentication (an authenticator app, with single-use recovery codes) from their profile. A passkey works both as the second factor after a password and on its own for passwordless sign-in. Custom fields, tags, and attachments add your own details, labels, and files to organizations; fields are typed, can be required, and can apply to just one client's subtree. Teams (Settings → Teams) group people and service accounts; roles granted to a team apply to every member, on top of their own. Notifications (the bell in the app, and email) tell people about changes to their account and access as they happen; each person chooses how they get them, except security notices, which always arrive both ways. For scripts and integrations, people create personal access tokens on their profile (scoped, expiring within a year), and administrators create service accounts under Settings → Service accounts: non-human identities that hold roles and use API tokens only. Send a token as `Authorization: Bearer hxp_…`. Under Settings → Single sign-on, administrators connect Microsoft Entra ID, Google, Okta, Authentik, Keycloak, or any OpenID Connect provider, restrict it to their email domains, optionally create accounts on first sign-in, and map group or app-role claims to roles (see ADR-019). Under Settings → Security, administrators can require two-factor authentication for everyone, for particular roles, or for particular organizations; people it covers who haven't set it up can only do that until they have. If someone loses both, an administrator can remove it under Settings → Users; only an owner can do that for another owner.

The preset chooses which feature modules start on: `automation` (none: Huxley as an automation engine only), `service-desk`, or `full`. Change them any time under Settings → Modules. Modules not yet released stay off until a release includes them.

Then open https://localhost (staff app) and https://portal.localhost (client portal). Set `HUXLEY_DOMAIN` and `HUXLEY_PORTAL_DOMAIN` in `.env` for real hostnames; Caddy obtains certificates automatically.

### Rotating the key-encryption key

Secrets are encrypted with per-provider-org data keys, which are in turn encrypted ("wrapped") by the KEK. To rotate the KEK: generate a new one, set it as `HUXLEY_CRYPTO__KEK` with the old one in `HUXLEY_CRYPTO__RETIRED_KEKS`, restart, run `huxley admin rewrap-keys`, then remove the retired key. The server refuses to start if any stored data key was wrapped by a KEK it doesn't have.

### Keeping the key-encryption key in Vault or OpenBao

Instead of holding the KEK in configuration, Huxley can ask HashiCorp Vault or OpenBao to wrap and unwrap data keys with a Transit key, so the KEK never leaves the key service:

```sh
bao secrets enable transit
bao write -f transit/keys/huxley        # aes256-gcm96
# Policy for Huxley: update on transit/encrypt/huxley and transit/decrypt/huxley.
```

Then set `HUXLEY_CRYPTO__VAULT__ADDRESS` and one way to sign in: `HUXLEY_CRYPTO__VAULT__TOKEN` (a periodic token), AppRole (`HUXLEY_CRYPTO__VAULT__APPROLE__ROLE_ID` and `..._SECRET_ID`), or Kubernetes (`HUXLEY_CRYPTO__VAULT__KUBERNETES__ROLE`). Optional: `..._MOUNT` (default `transit`), `..._KEY` (default `huxley`), `..._NAMESPACE`, `..._CA_CERT_FILE` for a private CA. Huxley checks at startup that it can wrap and unwrap with the key, and unwraps each organization's data key once per process, so Vault is not on the request path.

To move an existing install over, keep `HUXLEY_CRYPTO__KEK`, add the Vault settings and `HUXLEY_CRYPTO__ACTIVE=vault`, restart, run `huxley admin rewrap-keys`, then remove the KEK. After rotating the key inside Vault, `huxley admin rewrap-keys --all` moves data keys to its newest version, so older versions can be retired with `min_decryption_version`.

### Keeping the key-encryption key in Azure Key Vault

Create an RSA key (3072 or 4096 bits) in a Key Vault or Managed HSM and give Huxley's identity the *Key Vault Crypto User* role on it (wrap and unwrap only). Then set `HUXLEY_CRYPTO__AZURE_KEY_VAULT__VAULT_URL` (e.g. `https://contoso.vault.azure.net`) and, if the key isn't named `huxley`, `..._KEY`. Huxley signs in with managed identity by default (Container Apps, App Service, or a VM), with workload identity on AKS when `AZURE_FEDERATED_TOKEN_FILE` is set, or with an app registration's secret (`..._AUTH__TENANT_ID`, `..._AUTH__CLIENT_ID`, `..._AUTH__CLIENT_SECRET`). For a user-assigned identity set `..._AUTH__CLIENT_ID`. Rotate the key in Key Vault whenever you like: each wrapped data key remembers its key version, and `huxley admin rewrap-keys --all` moves them to the newest one. Moving from a local KEK works as for Vault, with `HUXLEY_CRYPTO__ACTIVE=azure_key_vault`.

### Configuration

The server reads, in increasing precedence: built-in defaults, `huxley.toml`, then `HUXLEY_<SECTION>__<KEY>` environment variables. Any variable can instead be supplied as a file by appending `_FILE` (for Docker and Kubernetes secrets).

| Variable | Default | Purpose |
|---|---|---|
| `HUXLEY_DATABASE__URL` | (required) | PostgreSQL connection URL |
| `HUXLEY_CRYPTO__KEK` | (required unless a key service is set) | Key-encryption key for secrets at rest: 32 random bytes, base64 (`huxley admin generate-kek`). Back it up: without it, encrypted data is unrecoverable. |
| `HUXLEY_CRYPTO__VAULT__ADDRESS` | (unset) | Keep the KEK in Vault or OpenBao Transit instead (see above) |
| `HUXLEY_CRYPTO__AZURE_KEY_VAULT__VAULT_URL` | (unset) | Keep the KEK in Azure Key Vault instead (see above) |
| `HUXLEY_CRYPTO__ACTIVE` | (the one configured) | `local`, `vault`, or `azure_key_vault`: which KEK wraps new data keys when several are set |
| `HUXLEY_AUTH__PUBLIC_URL` | (unset) | The staff app's URL as browsers see it, e.g. `https://huxley.example.com`. Enables passkeys, which are bound to its host name (changing the host later invalidates registered passkeys), and single sign-on, whose redirect URI is `<public URL>/login/oidc/callback`. |
| `HUXLEY_EMAIL__SMTP_URL` | (unset) | SMTP server for password reset, invitations, and verification, e.g. `smtps://user:pass@smtp.example.com` or `smtp://user:pass@host:587?tls=required`. Unset or empty: email is off. Links need `HUXLEY_AUTH__PUBLIC_URL`. |
| `HUXLEY_EMAIL__FROM` | (required with SMTP) | From address, e.g. `Huxley <huxley@example.com>` |
| `HUXLEY_STORAGE__URL` | `./data/objects` (image: `/var/lib/huxley/objects`) | Where attachments go: a directory, `s3://bucket[/prefix]` (AWS S3, DigitalOcean Spaces, MinIO), or `az://container`. Credentials and endpoints via `HUXLEY_STORAGE__OPTIONS__<NAME>`, e.g. `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, `AWS_ENDPOINT`. Back up local storage with the database. |
| `HUXLEY_STORAGE__MAX_UPLOAD_MB` | `25` | Largest attachment |
| `HUXLEY_CRYPTO__RETIRED_KEKS` | (empty) | Previous KEKs as a list, e.g. `[old1, old2]`, during rotation |
| `HUXLEY_ENGINE__CONCURRENCY` | `8` | Workflow jobs each worker process runs at once |
| `HUXLEY_ENGINE__LEASE_SECS` | `60` | How long a crashed worker's jobs wait before another worker takes them over |
| `HUXLEY_ENGINE__EGRESS__ALLOW_PRIVATE` | `false` | Let workflow HTTP steps reach private and loopback addresses. Link-local addresses (cloud metadata) stay blocked. |
| `HUXLEY_ENGINE__EGRESS__PRIVATE_HOSTS` | (empty) | Host names that may resolve to private addresses anyway, e.g. `[api.internal, "*.corp.example"]` |
| `HUXLEY_ENGINE__EGRESS__DENIED_HOSTS` | (empty) | Host names workflows may never reach |
| `HUXLEY_ENGINE__EGRESS__MAX_RESPONSE_BYTES` | `1048576` | Largest response body an HTTP step reads |
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
