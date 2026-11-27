# Huxley: Design Document

| | |
|---|---|
| **Status** | Draft v0.3 (all open questions resolved) |
| **Website** | usehuxley.io |
| **Date** | 2026-11-21 |
| **License** | MIT |
| **Companion** | `IMPLEMENTATION_PLAN.md` (phased roadmap) |

---

## 1. Summary

Huxley is a free, open-source, self-hostable platform for MSPs, MSSPs, and service desks. At its core it is an **automation and workflow engine**, similar in scope to n8n or Rewst. On top of that engine it adds the operational modules these businesses run on:

- **PSA**: ticketing, assets/CMDB, knowledge, contracts, change management, project management, inventory
- **Sales and procurement**: pipeline, CPQ, purchase and sales orders
- **Customer experience**: CSAT, NPS, client health
- **Analytics**: custom widgets, reports, dashboards
- **Billing**: invoice generation, synced to accounting systems (QuickBooks Online, Xero, etc.) through integrations
- **Client portal**
- **AI assistant and MCP server**

The core idea: **every module is built on the engine.** Mailbox polling, SLA escalation, AI triage, and technician "actions" such as *Create M365 User* are all system-provided workflows, forms, and integrations. Users can inspect, copy, and extend them. Because the platform runs its own processes through the same engine, any MSP can change how Huxley behaves without forking it.

### 1.1 Goals

1. **No lock-in.** Free (MIT), easy to deploy (Docker), runs on commodity infrastructure (DigitalOcean, Azure, bare metal).
2. **Automation first.** Workflows can be built three ways: on the visual canvas, as code, and through MCP/AI. All three edit the same underlying definition.
3. **Extensible through integrations.** Import an OpenAPI spec and get actions, auth, and data sync with no custom code.
4. **Multi-organization.** An org hierarchy (MSP → client → sub-org) where integrations, variables, and workflows are inherited.
5. **Secure and reliable by default.** Granular RBAC, encrypted secrets, durable execution, a full audit trail.
6. **Scales from 1 technician to large MSPs.** A single container works for small installs. Horizontally scaled services work for large ones.
7. **Modern, intuitive UX.** The fastest route to "the information I need" or "the action I need to take."

### 1.2 Non-goals (for now)

- An RMM agent or remote control. Huxley integrates with RMMs; it does not replace them. Scripts on customer networks run **through integrations** (RMM script execution, Intune, Azure Arc). A first-party Huxley agent is left open as a future possibility but is not on the roadmap. (See §6.9.)
- An accounting system. Huxley generates invoices and syncs them to the accounting platform (QuickBooks Online, Xero, etc.), which remains the system of record for payments, AR, and tax. (See §8.11.)
- Multi-tenant SaaS for unrelated MSPs. One instance serves one owning company, which may own several MSPs. (See §4.4.)
- Bundled hosted AI models. AI is bring-your-own-key (BYOK) or a self-hosted model endpoint.
- Native mobile apps in early phases. The web apps are responsive and installable as PWAs.

---

## 2. Guiding Principles

| Principle | What it means in practice |
|---|---|
| **Engine as the platform** | Modules emit domain events and expose core actions. Their automated behavior lives in system workflows, not in hidden code paths. |
| **One canonical definition** | Workflows, forms, integrations, and dashboards each have one versioned, serializable definition (JSON/YAML). The UI, code, and MCP all produce and consume it. |
| **Secure by default** | Deny-by-default authorization, encrypted secrets, SSRF protection, sandboxed scripts, audited changes. Insecure options are explicit opt-ins. |
| **Durable over fast** | A crash, deploy, or restart never loses or duplicates a workflow step's side effects without the user knowing. |
| **Minimal required infrastructure** | PostgreSQL is required. Valkey is optional for small installs and recommended at scale. Object storage can be the local filesystem. |
| **Boring, proven tech** | Mature crates and libraries. All dependencies must be MIT-compatible (no AGPL or source-available code). |
| **API first** | Everything the UI does goes through the public, versioned, OpenAPI-documented API. |

---

## 3. Technology Stack

| Layer | Choice | Notes |
|---|---|---|
| Backend language | **Rust** (stable, 2024 edition) | |
| Async runtime / HTTP | `tokio`, `axum`, `tower`, `tower-http` | |
| Database | **PostgreSQL 16+** | Extensions: `pgcrypto`, `ltree`, `pg_trgm`, `pgvector` (optional, for AI features) |
| DB access | `sqlx` (compile-time checked queries), `sqlx migrate` | Repository layer per module. No heavy ORM. |
| Cache / pub-sub / rate limits | **Valkey** (Redis-compatible) via `fred` or `redis` crate | Optional for single-node installs |
| Job queue | Postgres-backed queue (`SELECT … FOR UPDATE SKIP LOCKED`) | Transactional enqueue with domain writes. Valkey used only for wakeups and fan-out. |
| Templating / expressions | **`minijinja`** | Jinja2-compatible, sandboxed, written in Rust |
| OpenAPI parsing | `oas3` / `openapiv3` | Supports OpenAPI 3.0 and 3.1, plus a Swagger 2.0 converter |
| API docs / client gen | `utoipa` → OpenAPI → generated TS client | |
| AuthN | `argon2` (passwords), `openidconnect`, `webauthn-rs`, `totp-rs` | |
| AuthZ | **Cedar** (`cedar-policy`) behind an internal `Authorizer` trait | Rust-native, Apache-2.0, supports fine-grained ABAC |
| Crypto | `aws-lc-rs` / `ring`, `rustls` | AES-256-GCM envelope encryption |
| Object storage | `object_store` crate | S3, DO Spaces, Azure Blob, MinIO, local FS |
| Email | Microsoft Graph (REST), `async-imap`, `lettre` (SMTP), `mail-parser` | |
| Observability | **OpenTelemetry** (traces, metrics, logs via OTLP): `opentelemetry`, `opentelemetry-otlp`, `tracing-opentelemetry` | The single instrumentation standard. Optional Prometheus scrape endpoint. See §13.3. |
| Frontend | **SvelteKit** (Svelte 5 runes), TypeScript | |
| Canvas | **SvelteFlow** (`@xyflow/svelte`) | Workflow canvas (edges derived from the document, ADR-034) and CMDB dependency maps |
| UI kit | Tailwind CSS 4, `bits-ui` / `shadcn-svelte`, Lucide icons | |
| Data fetching | TanStack Query (Svelte), generated typed API client | |
| Code editing | CodeMirror 6 (Jinja, YAML, PowerShell, Python, JS/TS, JSON, Markdown) | One shared editor component everywhere code is edited; Monaco ruled out as too heavy (§6.7) |
| Charts | Apache ECharts or LayerChart | For the widget/dashboard builder |
| Script runners | Container images with PowerShell 7, Python 3, Node.js | Isolated from the core (§6.9) |
| Packaging | Docker (multi-arch), Docker Compose, Helm chart | |
| CI | GitHub Actions: `cargo clippy/test/deny/audit`, `pnpm lint/test`, Playwright, Grype container scan, Syft SBOM, cosign signing | Actions pinned to commit SHAs. Grype is used instead of Trivy, whose distribution was compromised in 2026. |

---

## 4. System Architecture

### 4.1 High-level components

```
                     ┌─────────────────────────────────────────────────────────┐
  Technicians ──────▶│  web (SvelteKit)        portal (SvelteKit)              │◀── Client contacts
  Admins             │  - app UI, canvas       - client portal                 │
                     └──────────────┬──────────────────────┬───────────────────┘
                                    │  HTTPS (REST + WS/SSE)│
  AI clients (MCP) ──▶ ┌────────────▼──────────────────────▼───────────────────┐
  External systems ──▶ │  huxley api  (Rust / axum, stateless, N replicas)      │
  (webhooks/callbacks) │  REST API · MCP server · webhook ingress · OIDC · WS    │
                       └───────┬────────────────────────┬───────────────────────┘
                               │                        │
                 ┌─────────────▼──────────┐   ┌─────────▼─────────┐
                 │ PostgreSQL (primary +  │   │ Valkey (optional) │
                 │ optional read replicas)│   │ cache, pub/sub,   │
                 │ data, queue, run state │   │ rate limits, locks│
                 └─────────────▲──────────┘   └─────────▲─────────┘
                               │                        │
          ┌────────────────────┴───────┬────────────────┴─────────┐
 ┌────────┴────────┐  ┌────────────────┴──┐  ┌────────────────────┴─┐
 │ huxley worker   │  │ huxley scheduler  │  │ huxley runner(s)     │
 │ executes steps, │  │ cron, timers, SLA │  │ sandboxed scripts:   │
 │ N replicas      │  │ clocks, leader-   │  │ pwsh / python / js   │
 │                 │  │ elected           │  │ (no DB access)       │
 └─────────────────┘  └───────────────────┘  └──────────────────────┘
                               │
                     ┌─────────▼─────────┐        ┌──────────────────────┐
                     │ Object storage    │        │ External systems     │
                     │ attachments, large│        │ (RMMs, cloud APIs):  │
                     │ payloads, exports │        │ on-prem exec via RMM │
                     └───────────────────┘        └──────────────────────┘
```

### 4.2 Process model

The backend is **one Rust binary (`huxley`) with role subcommands**, so small installs can run everything in one process and large installs can split roles:

```
huxley serve --roles all                # single-node / dev
huxley serve --roles api                # stateless HTTP tier
huxley serve --roles worker             # workflow step execution
huxley serve --roles scheduler          # timers/cron (leader-elected via PG advisory lock)
huxley runner                           # separate image, script sandbox host
huxley migrate | huxley admin …         # ops tooling
```

`huxley-cli` is a separate binary for developers. It handles push/pull/validate/test of workflows-as-code and talks to the API.

### 4.3 Communication

- **Browser ↔ API**: REST (JSON) under `/api/v1`. WebSocket or SSE carries live updates (run progress, ticket changes, presence). Live events fan out across API replicas via Valkey pub/sub, or via Postgres `LISTEN/NOTIFY` when Valkey isn't deployed.
- **API ↔ workers**: through the database. The API writes a run and enqueues jobs *in the same transaction* as the triggering change (transactional outbox). Workers claim jobs with `SKIP LOCKED`. A `NOTIFY` or Valkey ping wakes idle workers, so latency stays low without busy polling.
- **Worker ↔ runner**: gRPC (tonic) over mTLS. Runners never get database credentials. They receive a script, its inputs, and short-lived scoped credentials, then stream back logs and results.
- **Domain events**: modules publish events such as `ticket.created` and `asset.updated` to an `events` outbox table. A dispatcher matches them against event triggers and starts workflows.

### 4.4 Tenancy model

- **One instance = one owning company.** Huxley is not built as multi-tenant SaaS for unrelated MSPs, so there is no tenant layer above the org tree.
- **Multiple MSPs under one owner.** When a parent company owns several MSPs, each MSP is a `provider` org under the owning company's root org (§5.1). Through inherited settings and RBAC scoped to its subtree, each MSP can have its own:
  - branding and portal domain
  - mailboxes, queues, and SLA policies
  - numbering prefixes
  - staff

  Resources meant to be shared (integrations, workflows, scripts, product catalog) are defined at the root and inherited by every MSP.
- **Single-MSP installs** simply make the MSP the root org.
- **Isolation**: org scoping is enforced in three places: the repository layer, the authorization layer, and **PostgreSQL Row-Level Security** (RLS) as defense in depth. The app sets the caller's permitted org scope in a GUC per transaction. Staff of one MSP see another MSP's data only if they hold a role at a shared ancestor, such as the owning company.

---

## 5. Organization Hierarchy and Inheritance

### 5.1 Model

```
Instance (one owning company)
└── Organization: "Northwind Holdings"   (kind = provider, root; optional holding layer)
    ├── Organization: "Acme MSP"         (kind = provider, MSP brand)
    │   ├── Organization: "Contoso Ltd"  (kind = client)
    │   │   ├── "Contoso – EU"           (kind = client, sub-org/site)
    │   │   └── "Contoso – US"
    │   └── Organization: "Fabrikam"     (kind = client)
    └── Organization: "Blue Yonder IT"   (kind = provider, second MSP brand)
        └── …
```

A single-MSP install has no holding layer: the MSP itself is the root.

- Stored as an adjacency list (`parent_id`) plus a materialized `ltree` path for fast ancestor and descendant queries.
- `kind`: `provider` (the owning company, an MSP brand, or a division) or `client`. Clients map to "companies" in PSA terms. A client org is the customer record: contacts, sites, agreements, and assets attach to it.
- Moving an org re-parents its subtree, recomputes paths, and re-evaluates inherited resolutions. Moves are audited.

### 5.2 Inheritable resources

| Resource | Inheritance behavior |
|---|---|
| **Organization variables** | Resolved from the nearest ancestor that defines the key. A parent can mark a variable **locked**, which stops descendants from overriding it. |
| **Integration connections** | A connection (e.g., an "Acme Partner Center" credential) defined at a parent can be **shared down** to all or selected descendants. A child can override it with its own connection, such as the client's own M365 tenant. |
| **Workflows** | Workflows can be **published down**. Descendants may run them, and if allowed, *fork* them locally. Bindings (which connection or variable to use) resolve in the context of the org that runs the workflow. |
| **Forms / Actions** | Same as workflows. Visibility can be limited to particular descendants or org tags. |
| **Scripts and templates** | Shared libraries inherit down. Read-only unless forked. |
| **Settings** (SLA calendars, business hours, ticket defaults, branding) | Nearest-ancestor wins, with optional lock. |
| **RBAC role assignments** | An assignment made at an org applies to that org and its descendants unless it is marked `this_org_only`. |

### 5.3 Resolution algorithm

To resolve `X` at org `O`, look up the candidate definitions at `O.path` and every ancestor (one indexed query on `ltree @>`), sort by depth descending, and choose the nearest unless a shallower one is `locked`. Results are cached per `(org, resource-kind)` in Valkey or in-process. The cache is invalidated by version bumps.

---

## 6. Automation Engine (Core)

### 6.1 Concepts

| Concept | Description |
|---|---|
| **Workflow** | A named, versioned, directed graph of **steps** with one or more **triggers**. Lifecycle: `draft` → `published` (immutable version) → `archived`. Runs always pin to a published version (or the draft, when testing). |
| **Trigger** | What starts a run: manual, form submission, schedule (cron/interval), webhook, domain event, integration poll, email received, sub-workflow call, MCP/AI tool call. |
| **Step (node)** | One unit of work: an integration action, HTTP request, script, transform, control flow, human task, AI call, core platform action, and so on. |
| **Run** | One execution of a workflow version: its trigger payload, context, step records, status, and timing. |
| **Step run** | One attempt of one step: rendered inputs (secrets redacted), output, logs, error, duration, attempt number. |
| **Context** | The data available to Jinja expressions: `trigger`, `steps.<id>.output`, `vars` (org variables), `org`, `actor`, `run`, `env`, `loop`. |
| **Connection** | A stored, encrypted credential instance for an integration, scoped to an org. |
| **System workflow** | A workflow shipped by Huxley or a module (e.g., *Poll M365 mailbox*). Marked `system`. It can be viewed, copied, and overridden per org, but not edited in place. Upgrades ship new versions. |

### 6.2 Canonical workflow definition (HWF)

Every workflow serializes to a **Huxley Workflow Format (HWF)** document, stored as JSON and authored as YAML. The canvas, the code/CLI path, and MCP all produce and consume HWF. Canvas layout lives in a separate `ui` block, so code edits don't fight with node positions.

```yaml
apiVersion: usehuxley.io/v1
kind: Workflow
metadata:
  key: m365-create-user            # stable identifier, unique per owning org
  name: Create M365 User
  description: Creates a user in the client's M365 tenant and assigns a license.
  tags: [m365, onboarding]
spec:
  inputs:                           # JSON Schema; also drives the form generator
    type: object
    required: [firstName, lastName, licenseSku]
    properties:
      firstName: { type: string }
      lastName:  { type: string }
      licenseSku: { type: string, x-huxley-options: { dataset: m365_licenses, value: skuId, label: name } }
  triggers:
    - type: form
      form: m365-create-user-form
    - type: mcp_tool
  steps:
    - id: build_upn
      type: transform
      output: "{{ (inputs.firstName ~ '.' ~ inputs.lastName) | lower }}@{{ vars.m365_primary_domain }}"
    - id: create_user
      type: integration.action
      connection: "{{ connections.microsoft_graph }}"   # resolved in the running org's context
      action: microsoft-graph/users.create
      with:
        displayName: "{{ inputs.firstName }} {{ inputs.lastName }}"
        userPrincipalName: "{{ steps.build_upn.output }}"
        passwordProfile: { password: "{{ secrets.generate(24) }}", forceChangePasswordNextSignIn: true }
      retry: { max_attempts: 3, backoff: exponential, on: [429, 5xx] }
    - id: assign_license
      type: integration.action
      action: microsoft-graph/users.assignLicense
      with: { id: "{{ steps.create_user.output.id }}", addLicenses: [{ skuId: "{{ inputs.licenseSku }}" }] }
    - id: note_ticket
      type: core.ticket.add_note
      if: "{{ trigger.ticket_id is defined }}"
      with: { ticket_id: "{{ trigger.ticket_id }}", body: "Created {{ steps.build_upn.output }}", internal: true }
  outputs:
    upn: "{{ steps.build_upn.output }}"
  on_error:
    - id: notify
      type: core.notify
      with: { to: "role:service-desk-leads", message: "User creation failed: {{ error.message }}" }
ui:
  nodes: { build_upn: { x: 120, y: 80 }, create_user: { x: 120, y: 200 } }
```

**Flow semantics.** Steps run in order. Control-flow nodes own nested blocks of steps; there are no free-form edges (ADR-020):

| Node | Purpose |
|---|---|
| `if` / `switch` | Conditional branching on Jinja expressions |
| `foreach` | Iterate a list with `concurrency: N`, collecting results |
| `parallel` / `join` | Fan-out and fan-in, with `all`/`any`/`n-of` join policies |
| `try` / `catch` / `finally` | Scoped error handling |
| `subworkflow` | Call another workflow (sync or async) with typed inputs and outputs |
| `wait` | Delay for a duration or until a timestamp |
| `wait_for_callback` | Park until an external system calls a signed URL |
| `approval` / `human_task` | Park until a user acts, via a form, the ticket UI, email link, or Teams/Slack |
| `return` / `fail` | End the run with outputs or an error |

**Step types**: `integration.action`, `http.request`, `script` (library or inline), `transform` (Jinja → value), `set_var` (run-scoped), `dataset.query` / `dataset.upsert`, `ai.prompt` / `ai.classify` / `ai.extract`, `core.*` (platform actions such as create ticket, update asset, send email, notify), `mcp.call` (call an external MCP server tool, later phase).

### 6.3 Expressions: Jinja via minijinja

- Any string field may contain Jinja. Fields with the shape `"{{ expr }}"` evaluate to **typed values** (objects, lists, numbers), not just strings.
- Custom filters and functions: `to_json`, `from_json`, `jmespath`, `date`/`dateadd`/`tz`, `regex_*`, `b64*`, `hash`, `uuid`, `slugify`, `secrets.generate`, `default_if_none`, plus a `lookup(dataset, …)` helper.
- **Sandboxed**: no filesystem, no network, bounded by fuel/step limits and output size limits.
- `secrets.*` values are **taint-tracked**. They can be passed to steps but are redacted from logs, run history, and AI context. Rendering a secret into a log-bound field is replaced with `«redacted»`.
- The canvas has a live **expression editor**: autocomplete over the context schema (built from upstream step output schemas), inline preview against the last test run's data, and type hints.

### 6.4 Execution model and durability

- **Journal-based durable execution.** Each step's completion (output or error) is committed to `step_runs` before the next step is scheduled. Resuming after a crash replays the graph walk, skips completed steps, and re-executes only in-flight steps.
- **At-least-once step execution with idempotency support.** Each step attempt gets a deterministic `idempotency_key = hash(run_id, step_path, attempt_group)`. It is exposed to integrations (sent as an `Idempotency-Key` header when the API supports it) and to scripts.
- **Job queue** (`jobs` table): `id, kind, run_id, step_path, run_at, priority, attempts, locked_by, locked_until, org_id, concurrency_key`. Workers claim with `SKIP LOCKED`, heartbeat by extending `locked_until`, and the reaper reclaims expired locks.
- **Retries**: per-step policy (`max_attempts`, backoff `fixed|exponential|jitter`, `retry_on` status or error classes, `timeout`). Exhausted retries route to `catch`/`on_error` or fail the run. Failed runs can be **resumed from the failed step** after a fix.
- **Fairness and limits**: per-org and per-workflow concurrency limits, per-connection rate limits (token bucket in Valkey, or Postgres when Valkey is absent). Weighted fair scheduling stops one noisy client org from starving others.
- **Payload handling**: step inputs and outputs above a threshold (default 256 KB) spill to object storage, leaving a reference in Postgres. A hard cap per step output is configurable.
- **Timers**: `wait` and scheduled triggers are rows with `run_at`. The leader-elected scheduler enqueues due work, so there is no in-memory timer state.
- **Cancellation**: cooperative. Runners receive a cancel signal and in-flight HTTP calls are aborted.
- **Versioning**: runs pin to a version. Publishing a new version never alters in-flight runs.
- **Retention**: run history is partitioned by month (declarative partitioning). Retention is configurable per org and per workflow, with defaults like 30 days of full payloads and 1 year of run metadata.

### 6.5 Triggers in detail

| Trigger | Details |
|---|---|
| **Manual** | From the UI, the API, the client 360 "Actions" panel, or the CLI. |
| **Form** | A form submission. Fields map to workflow `inputs` (§6.8). |
| **Schedule** | Cron (with timezone) or interval. Can be evaluated per org: "run for each client org that has variable X set". |
| **Webhook** | `POST /hooks/{public_id}`. Auth options: HMAC signature (configurable header and algorithm), bearer token, basic auth, mTLS, IP allowlist. Response mode is `async` (202 + run id) or `sync` (wait up to N seconds and return the workflow's `outputs` as the HTTP response). Replay protection uses timestamp plus nonce for HMAC. |
| **Callback** | `wait_for_callback` creates a single-use, signed, expiring URL (`/callbacks/{token}`). A request to it resumes the step with the request body as output. |
| **Domain event** | `ticket.created`, `ticket.status_changed`, `asset.warranty_expiring`, `contract.renewal_due`, `csat.received`, etc. Supports filter expressions. |
| **Integration poll** | Periodically calls a list action and starts runs for new or changed items. Cursor, watermark, and dedupe state is kept per connection. |
| **Email** | Mail ingestion (§8.4) emits `email.received`, which system workflows consume. |
| **MCP / AI** | Workflows marked `mcp_tool` are exposed as tools to MCP clients and the assistant, subject to RBAC. |

### 6.6 Reusable building blocks

- **Script library**: versioned scripts (PowerShell, Python, JavaScript/TypeScript, Bash) with a typed parameter schema, a declared output schema, a runner requirement (e.g., `pwsh` + module `ExchangeOnlineManagement`), and tags. Scripts can be shared down the org tree.
- **Templates**: workflow, form, and dashboard templates. One-click "use template" clones into a draft with bindings to resolve.
- **Huxley Packs**: a signed, versioned bundle (`.hxpack`, a zip with a manifest) of integrations, workflows, scripts, forms, datasets, and dashboards. Import, export, and upgrade via the UI or CLI. System modules ship their automations as built-in packs. A community pack registry backed by a public git repo comes later.
- **Organization variables**: typed (`string`, `number`, `bool`, `json`, `secret`, `connection-ref`), with description, validation schema, inheritance, and lock (§5). Secret-typed variables are encrypted like connections.

### 6.7 Three ways to build

1. **Canvas (SvelteFlow)**
   - Node palette with search, grouped by integration, core, and control flow. Integration actions come with icons from imported specs.
   - Auto-layout (ELK/dagre), minimap, groups/frames, comments/sticky notes, copy/paste between workflows (HWF fragments on the clipboard).
   - A **schema-aware input panel** for each node. Forms are generated from the action's JSON Schema, and every field has a "switch to expression" toggle.
   - **Test mode**: run the whole workflow or *from this node*, with pinned sample data. Each node shows its I/O inline, and the run timeline overlays the canvas.
   - Validation runs as you type: unresolved references, type mismatches, missing connections.
   - Version diff view comparing graphs side by side.
2. **Code**
   - HWF YAML in a repository. `huxley-cli` provides `validate`, `test` (with fixtures), `push`, `pull`, `diff`, and `run`.
   - **Git sync** (per org): bidirectional sync with a Git repo (GitHub, GitLab, Azure DevOps). Pushes to a branch publish versions, and UI changes can open PRs. The JSON Schema for HWF is published so editors get autocomplete.
   - An in-app code view (CodeMirror YAML) that round-trips with the canvas.
   - **TypeScript SDK** (`@usehuxley/sdk`, plan Phase 7) for teams who prefer real code to YAML:
     - Typed builders for steps, control flow, triggers, forms, and inputs.
     - `huxley-cli codegen` pulls the integrations installed on an instance and generates TypeScript types for their actions. Action inputs and outputs are type-checked, with editor autocomplete.
     - Jinja stays the expression language at runtime. The SDK provides typed helpers that emit Jinja (e.g., `ref(steps.createUser).output.id`), and raw template strings remain available.
     - **Compile-time only**: `huxley-cli build` compiles TS to HWF with deterministic output, so Git diffs stay clean. The engine only ever executes HWF. The SDK adds no runtime, no new sandbox, and no second execution path.
     - Works with `huxley-cli test`, push, and Git sync exactly like YAML. Pack authors can write packs in TS.
3. **MCP / AI (BYOK)**
   - Huxley exposes an **MCP server** (§10.2) with tools like `list_integrations`, `describe_action`, `create_workflow_draft`, `update_workflow_draft`, `validate_workflow`, `test_run`, and `get_run`. Any MCP client (Claude Desktop, Claude Code, IDEs) using the user's own AI key can build and debug workflows.
   - The built-in **AI assistant** uses the same tools, with the provider key configured by the org.
   - AI-created changes always land as **drafts**. Publishing requires a human with permission, and every change is attributed in the audit log as `actor=user, via=mcp`.


**The in-app code editor.** Everywhere Huxley takes code, it uses one shared editor component built on **CodeMirror 6**, never a plain text box. Monaco was ruled out as too heavy. CodeMirror is modular (only the languages in use are loaded, on demand), MIT-licensed, works with screen readers and on phones, and supports completion, linting and themes through extensions.
- **Where:**
  - script source and parameters
  - expression fields in the step panel and the workflow's YAML code view
  - ticket and email templates, canned responses
  - form and dataset JSON Schemas, integration overlays
  - pack manifests
- **Languages:** PowerShell, Python, JavaScript/TypeScript, YAML, JSON, Markdown, and Jinja (HWF expressions, including Jinja inside YAML strings).
- **Smarter than a text box:**
  - Syntax highlighting, bracket matching, auto-indent, search and replace, multiple cursors, code folding.
  - **Context-aware completion:**
    - in expressions: `inputs.*`, `steps.<id>.output.*` (from the steps before this one and their output schemas), `vars.*`, `trigger.*`, organization variables, and the expression filters and functions
    - in scripts: the declared parameters, plus each language's keywords
    - in workflow YAML: step types and fields from the published HWF JSON Schema
  - **Inline diagnostics** from the same server-side validators the API uses (HWF validation, expression parsing, JSON Schema), debounced as you type. The editor and the API never disagree.
  - Hover documentation for steps, filters and functions. An expression preview evaluates against the last test run's data.
- **Fits the app:** colours come from the design tokens (light and dark). Read-only and diff modes are used for version history and pack upgrade previews. Escape then Tab leaves the editor, so the keyboard isn't trapped.
- **Size:** the editor loads only on pages that need it, and each language loads when first used. Pages without code don't pay for it.

### 6.8 Forms

- A **form builder** with drag and drop. The underlying definition is JSON Schema + a UI schema.
- Field types: text, rich text, number, date/time, select/multi-select, user/contact/org/asset pickers, file upload, password/secret, table/repeater, section, conditional group, markdown help text.
- **Dynamic options** sourced from an integration action, a dataset query, or a core entity query, evaluated in the submitter's org context. Fields can depend on other fields (cascading selects).
- Validation: schema rules, regex, cross-field Jinja rules, and optional server-side "validate" workflows.
- **Audiences**: internal (technicians, filtered by RBAC), client portal (contacts, filtered by portal RBAC), or public link (optional captcha, rate limiting, expiry).
- Submissions are stored (configurable redaction) and linked to their run. The submitter sees status and results if allowed.
- **Actions** (client 360): a form flagged `action` and bound to a workflow appears in the technician's client view. Context such as `org`, `contact`, `asset`, or `ticket` is injected automatically, so the technician fills in only what's needed.

### 6.9 Script execution and sandboxing

- Scripts **never run inside the API or worker process**. Workers dispatch to a pool of **runners**:
  - `runner-pwsh`: PowerShell 7 with a curated module set (Microsoft.Graph, ExchangeOnlineManagement, PnP) plus per-org extra modules allowed by an admin.
  - `runner-python`: CPython with a curated base, plus an admin-approved requirements lock per script.
  - `runner-js`: Node.js LTS, whose permission model restricts the filesystem, subprocesses and the network. (Deno was the first choice. Its team announced the end of its development in October 2026.)
- Each execution runs in an **ephemeral, non-root, read-only-rootfs container or microVM sandbox** (gVisor/Firecracker when available, plain hardened container otherwise). By default the runner starts it with **rootless Podman** inside its own container; installs on Kubernetes can have runners start **Jobs** instead (with a gVisor runtime class where available). Runners never get the Docker socket. It has CPU, memory, and time limits, a network egress policy (inherits the SSRF policy, §12.6), and no access to Huxley's database or secrets other than what's injected for that step. **Egress is enforced, not advisory:** the sandbox has no network of its own, and its only way out is the runner's HTTP(S) egress proxy, which applies the policy. HTTP(S) is the only protocol that gets out; on-prem protocols go through integrations (below).
- Credentials are injected as short-lived environment variables or files. They are scrubbed from output streams.
- **On-prem execution goes through integrations.** Runners execute scripts that act on cloud APIs. Work *inside* a customer network (AD, on-prem Exchange, file servers, endpoints) is done by calling an integration that already has a presence there. Examples:
  - An RMM's "run script on device" action (NinjaOne, Datto RMM, ConnectWise Automate, N-able), with the result collected by polling or `wait_for_callback`.
  - Intune remediations.
  - Azure Arc Run Command.

  Workflows stay agent-free, and MSPs use the tooling they've already deployed (§7.3).
- **Future possibility: a Huxley agent.** It is not on the roadmap. The worker ↔ runner protocol is kept transport-agnostic (gRPC over mTLS, where the runner pulls work) so a first-party agent could plug in later. It would connect outbound and run signed scripts, without changes to HWF or the engine.

---

## 7. Integrations Framework

### 7.1 OpenAPI import

1. An admin uploads or links an **OpenAPI 3.x spec** (Swagger 2.0 is auto-converted).
2. Huxley parses it into an **Integration Definition**:
   - **Actions**: one per operation (`operationId` or a generated name). Each action has an input schema (path, query, header, and body params merged) and an output schema (2xx response). Actions are grouped by tag.
   - **Auth schemes**: mapped from `securitySchemes` (API key in header/query/cookie, HTTP basic/bearer, OAuth2 client-credentials, OAuth2 authorization code with PKCE, OpenID Connect discovery). Additional custom schemes cover non-standard APIs, e.g., a token-exchange step (a mini-workflow) or AWS SigV4.
   - **Servers**: base URLs with variables, such as region or tenant subdomain, which become connection fields.
3. An **overlay editor** lets admins improve the import without editing the source spec: rename and hide actions, set friendly labels, choose pagination strategy (cursor, offset, link header, next-URL in body), configure rate limits, mark list actions as "pollable" and the item ID field, set retry rules, and define *resource models* used for datasets. Overlays are stored separately, so re-importing a newer spec version keeps customizations (diff shown on upgrade).
4. The result is a versioned integration. It can be exported as part of a pack.

### 7.2 Connections

- A connection is an instance of an integration's auth scheme plus server variables, owned by an org and optionally shared down (§5.2).
- OAuth2 authorization-code flows run in the UI with a consent redirect. Refresh tokens are rotated and stored encrypted. The refresh is serialized per connection with a distributed lock, so concurrent refreshes can't clobber each other.
- **Health checks**: an optional "test" action per integration. Connection status shows on dashboards, and expiring credentials raise notifications.
- **Multi-tenant partner patterns**: built-in support for the MSP pattern of one partner credential plus a per-client tenant ID, e.g., Microsoft GDAP/Partner Center, where a parent connection plus a child org variable equals the effective connection.

### 7.3 Built-in integrations

These ship as packs, built on the same OpenAPI mechanism with overlays: **Microsoft Graph / M365** (users, groups, licenses, mail, Teams), **Microsoft Partner Center / GDAP**, **Entra ID** (also used for OIDC), **Azure Resource Manager**, **Google Workspace**, plus later the common MSP tools (RMMs such as NinjaOne, Datto, ConnectWise Automate, N-able; IT Glue/Hudu; distributors such as Pax8, Ingram, TD SYNNEX; Dell/Lenovo/HP warranty APIs; QuickBooks Online/Xero). The community can add more as packs.

**On-prem execution through integrations.** RMM packs include script and job execution actions (run a script on a device or group, get the job's status and output). Workflows can therefore act inside customer networks without a Huxley agent (§6.9). Intune remediations and Azure Arc Run Command cover cloud-managed devices.

---

## 8. Platform Modules

All modules can be turned off (§8.0) and share the same patterns:

- Entities are owned by an org and protected by RBAC.
- Entities support **custom fields** (typed, per entity type, with optional scope to an org subtree), **tags**, **attachments**, an **activity timeline**, and **audit**.
- Every create, update, or state change emits a **domain event**.
- Every user-facing operation is also a **`core.*` workflow action**.

### 8.0 Feature modules and toggles

Everything outside the core platform is a **feature module** that an administrator can turn on or off. An MSP that only wants an automation engine runs Huxley with every module off and never sees ticketing, sales, or billing.

**Always on (core):** organizations, users, RBAC, audit, settings, the automation engine (workflows, triggers, scripts, forms, webhooks, callbacks), integrations and connections, organization variables, datasets, packs, the AI assistant and MCP server, notifications, and analytics over core and dataset data.

**Toggleable modules:**

| Module | Covers | Requires |
|---|---|---|
| `crm` | Companies, contacts, sites, client 360 view | none |
| `servicedesk` | Tickets, queues, SLAs, mailbox ingestion, dispatch | `crm` |
| `cmdb` | Assets, relationships, dependency map | `crm` |
| `knowledge` | Knowledge base, client documentation | none |
| `contracts` | Agreements, warranties, product catalog | `crm` |
| `change` | Change and problem management | `servicedesk` |
| `projects` | Projects, templates, resource planning | `crm` |
| `inventory` | Stock, warehouses, purchase orders | `contracts` |
| `sales` | Pipeline, CPQ, sales orders | `crm`, `contracts` |
| `cx` | CSAT, NPS, client health | `servicedesk` |
| `billing` | Invoice generation, accounting sync | `contracts` |
| `expenses` | Expense tracking, reimbursement, billable expenses (§8.16) | none |
| `portal` | Client portal | `crm` |
| `workforce` | Skills, certifications, availability, timesheets (§8.15) | none |

**How a module plugs in.** Each module registers a manifest with the platform's module registry:
- its permissions
- its API routes
- its navigation entries and pages
- its domain events and `core.*` workflow actions
- its built-in pack of system workflows
- its search and reporting data sources

The registry is the single place that decides whether any of these are active.

**What turning a module off does:**
- Its navigation, pages, and command-palette entries disappear.
- Its API routes return `404` with problem type `module-disabled`, and its MCP tools are withdrawn.
- Its permissions are hidden from role editors, and its `core.*` actions are unavailable to new workflow versions.
- Its system workflows are paused, and its data sources leave the report builder.
- **Data is kept.** Turning a module back on restores everything as it was. Removing a module's data is a separate, explicit, audited admin action.

**Rules:**
- Enabling a module also enables what it requires, after confirmation.
- A module can't be disabled while an enabled module depends on it.
- Published workflows that call a disabled module's actions are flagged in the UI and fail with a clear error, rather than failing silently.
- Module settings are instance-wide by default and can be narrowed per provider org. For example, under one owner, one MSP brand might use the service desk while another runs automation only.
- Enabling and disabling require the `platform:modules:manage` permission and are audited.

**Setup.** `huxley admin bootstrap` and the first-run setup offer presets: *Automation only* (no modules), *Service desk* (`crm`, `servicedesk`, `cmdb`, `knowledge`, `portal`), and *Full PSA* (everything). Modules can be changed later at any time.

### 8.1 Companies, contacts, sites (CRM base)

- Client orgs *are* companies. Data: **sites** (addresses, hours, timezone), **contacts** (people at the client, optionally linked to a portal user and to directory identities like the Entra object ID), **company types/statuses**, and relationships (vendor, partner, prospect).
- A contact can belong to multiple companies with a primary.
- **Client 360 view** (technician): a header with health score, SLA status, and open items. Tabs: overview, tickets, assets, contacts, contracts, documents/KB, sales, invoices, runs, **Actions** (forms), and integration-sourced panels (e.g., M365 license summary from a dataset).

### 8.2 Service desk / ticketing (ITIL-aligned)

- **Ticket types**: Incident, Service Request, Problem, Change (§8.6), plus custom types. Each type has its own status workflow (status machine with allowed transitions and required fields per transition), forms, and templates.
- **Service queues, not boards**: tickets belong to a **queue** (e.g., *L1 Service Desk*, *Projects*, *Security Ops*). Queues define membership (technicians/teams), default SLA, routing rules, intake channels, and visibility. Technicians work from **My Work** (assigned to me), **queue views** (saved, shareable filtered lists with columns and sort), and an optional kanban *visualization* of a view (a visualization only, not the ownership model).
- **Priority matrix**: impact × urgency → priority, configurable.
- **SLA engine**: SLA policies with first-response, next-response, and resolution targets per priority. **Business-hours calendars** (holidays, timezone per site/company), pause-on-status (e.g., *Waiting on Client*), warnings and breach events (`sla.warning`, `sla.breached`) feeding escalation workflows. The scheduler drives SLA clocks.
- **Ticket features**: notes (public/internal/resolution), email threading, time entries (billable, work type, linked to agreement), tasks/checklists, parent/child and related tickets, merge, split, bulk edit, watchers/CC, attachments, a **collision indicator** (who's viewing or typing), canned responses with Jinja, macros (run a workflow from a ticket), and ticket templates.
- **Assignment and dispatch**: manual, round-robin, load-balanced, skill-based, or AI-assisted (§8.8), configured as a system workflow per queue. Skill-based and AI dispatch draw on people's skills, certifications and availability (§8.15). Without the `workforce` module, dispatch falls back to load and round-robin. A dispatch board/calendar provides scheduling.
- **Intake channels**: email (§8.4), portal, forms, API, webhooks, phone integrations (via workflows), chat/Teams (later).

### 8.3 Notifications

- A central notification service with channels: in-app (real-time), email, Teams/Slack (via integration actions), web push.
- User preferences set per event type. Rules are themselves system workflows, so admins can customize them.

### 8.4 Mailbox ingestion (M365 and IMAP)

- **Mailbox connections**: M365 via Graph (app registration, either per-mailbox delegated or application access policy scoped) or IMAP/SMTP with OAuth2 XOAUTH2 or a password.
- System workflow **"Poll mailbox"**: Graph delta queries (with optional change notifications/subscriptions for near-real-time) or IMAP IDLE with a polling fallback. It emits `email.received` with the parsed message (headers, bodies, attachments to object storage).
- System workflow **"Email to ticket"**:
  - Threading by `In-Reply-To`/`References`, a subject token `[HX#12345]`, and a hidden reply marker.
  - Sender → contact → company match, with an unknown-sender policy.
  - Spam and auto-reply detection, plus loop protection (rate and header checks).
  - Strips quoted replies and signatures (configurable) and creates a ticket or adds a note.
- Outbound mail goes through Graph `sendMail` or SMTP, with per-queue "from" addresses and DKIM-friendly configuration guidance.

### 8.5 Assets, configuration and knowledge management

- **Assets / Configuration Items (CIs)**: CI types (workstation, server, network device, SaaS tenant, license, domain, certificate, application, …). Each type has a custom field schema, lifecycle status, owner/contact, site, and warranty.
- **Relationships and dependency mapping**: typed edges (`runs_on`, `depends_on`, `connects_to`, `hosts`, `licensed_by`, `backs_up`). Visualized as an interactive **SvelteFlow map** with impact analysis ("what breaks if this goes down?"), used by change management and incident correlation.
- **Discovery and sync**: assets are populated from integrations (RMM, M365, Azure) via **dataset → asset mapping rules**, with match keys (serial, MAC, hostname, object ID), conflict policy, and a provenance record per field.
- **Knowledge base**: articles with rich text and markdown, versioning, review/approval workflow, visibility (internal, client-specific, portal public), categories, linked to tickets, assets, and companies. Full-text search plus semantic search (pgvector). **Auto-documentation** (§8.8) can draft articles from resolved tickets.
- **Documentation / passwords**: a structured client documentation area. A credential vault is optional, uses the same envelope encryption, and logs every reveal as an audit event.

### 8.6 Change management

- Change types: standard (pre-approved templates), normal, emergency.
- Fields: risk assessment (questionnaire → risk score), impact (affected CIs from the dependency map), implementation, backout, and test plans, plus schedule windows.
- **Approvals / CAB**: approval policies (by risk, type, affected client) built on `approval` steps. Approvers can be internal or client contacts via the portal or email links.
- **Change calendar** with freeze/blackout windows and conflict detection.
- Post-implementation review. Changes link to the incidents and problems they resolve or cause.

### 8.7 Contracts, warranties, stock and inventory

- **Agreements/contracts**: managed services (per user/device/flat), block hours/retainers, time & materials, project. Coverage rules (which work types and CIs are covered), rate cards, and SLA policy binding. **Recurring billing lines** feed billing (§8.11). Renewal tracking emits `contract.renewal_due`.
- **Warranties**: per asset (start, end, type, provider). The *Warranty lookup* system workflow uses vendor integrations, and `asset.warranty_expiring` events drive renewal opportunities.
- **Product catalog**: products/services, SKUs, vendors, cost/price, bundles.
- **Inventory**: warehouses/locations (including technician vehicles), stock levels, serial/lot tracking, movements (receive, transfer, allocate to ticket/project/order, consume, return, RMA), reorder points emitting `stock.low`.

### 8.8 AI capabilities

All AI features go through a **provider abstraction** (BYOK). Supported providers include Anthropic, OpenAI, Azure OpenAI, and OpenAI-compatible endpoints such as Ollama or vLLM for fully self-hosted setups. Each org configures its provider; children can inherit it.

**Each AI feature is a system workflow** using `ai.*` steps, so MSPs can tune prompts, swap models, add guardrails, or turn features off.

| Feature | Behavior |
|---|---|
| **Sentiment analysis** | On each inbound message, store sentiment score, label, and confidence. Trend is aggregated per ticket, contact, and company. |
| **Triage** | Suggest or apply ticket type, category, priority (impact/urgency), and queue. Modes: suggest-only, or auto-apply above a confidence threshold. |
| **Dispatch** | Recommend the assignee using skills and certifications (§8.15), current load, availability/schedule and time off, past resolution history, and client familiarity. Certifications a ticket requires are hard constraints; the rest are weighed. Each recommendation shows its reasons. |
| **Skill gaps** | Flag work no available person is qualified for, and certifications expiring that queues or partner programs depend on. |
| **Ticket insights** | Summary, extracted entities (users, devices, error codes), similar past tickets, suggested KB articles, suggested next actions or Actions (forms). |
| **Auto-documentation** | On resolve, draft a resolution summary, time-entry notes, and an optional KB article draft (enters the review workflow). Draft time entries from a technician's ticket activity for them to confirm on their timesheet. |
| **Client health** | See §8.9. |
| **AI assistant** | Chat in the app (side panel, context-aware of the current page: ticket, client, workflow). Uses Huxley's MCP tools under the **user's own permissions**. Can answer questions, run Actions (with confirmation), and build workflows. |

Guardrails:

- PII and secret redaction before prompts (configurable).
- Per-org opt-in for each feature.
- Token and cost budgets per org.
- Every AI call is logged (prompt metadata and token counts; full prompts optional).
- Every AI-applied change is labeled and reversible.
- **Embeddings** in pgvector power similar-ticket and KB search.

### 8.9 Customer experience: CSAT, NPS, client health

- **CSAT**: a survey sent on ticket resolution (configurable sampling and per-company opt-out), 1–5 or thumbs, with comments. One-click email links plus portal.
- **NPS**: periodic campaigns per company/contact list (system scheduled workflow).
- **Client health score**: a weighted, configurable model over sentiment trend, CSAT average and trend, NPS, SLA breach rate, ticket volume anomalies, reopen rate, overdue invoices, and contract renewal proximity. Each company gets a score, trend, and *reasons*. An `client.health_declining` event lets account managers be alerted or create follow-up tasks automatically.

### 8.10 Sales, CPQ and procurement

- **Leads → Opportunities**: lead capture (forms, email, API), qualification, conversion to opportunity plus company/contact.
- **Pipeline**: configurable stages with probability, a kanban plus list view, a stage-change history, and win/loss reasons.
- **Activities and communications**: tasks, calls, meetings, and emails logged against leads, opportunities, and companies. Email logging uses Graph/IMAP sync plus a BCC address. Meetings can be logged from the synced calendars (§8.15).
- **Forecasting**: weighted pipeline by period, rep, and team, plus commit/best-case categories and quota attainment.
- **Quoting (CPQ)**:
  - Quote builder from the product catalog and bundles, with distributor pricing lookups via integrations.
  - Margin and markup rules, recurring vs one-time lines, options/alternates, and branded quote templates (HTML → PDF).
  - A quote version history and a client acceptance page (portal/public link with e-signature capture and audit trail).
  - **Internal review and approval before a quote is sent.** The review model is shared with invoices (§8.11):
    - **When it's needed is customizable.** Rules on the quote decide whether review is needed and who reviews it: total, margin or discount past a threshold, product categories, a new client, the sales rep, or any field. With no rule matching, a quote can go straight out.
    - **The approval itself is a workflow.** It's a built-in pack using the approval step, like change approvals (§8.6), so chains, parallel reviewers, levels, escalations and timeouts are editable like any workflow.
    - **A review thread on the quote:** comments, on the whole quote or on a line, with `@mentions` that notify (in the app and by email). Threads can be resolved.
    - Reviewers approve, request changes or reject. A resubmitted quote shows what changed since the reviewed version.
    - **It can't be sent until approved,** except by someone with an override permission, which is recorded.
    - Every step is in the quote's audit trail, and raises events (`quote.submitted_for_review`, `quote.approved`, `quote.changes_requested`, `quote.rejected`).
- **Orders**: accepted quote → **Sales Order** → generates **Purchase Orders** to vendors (consolidated by vendor) → receiving into inventory → allocation/fulfillment → billing. Recurring lines can create or update agreements.
- **Presales scoping for projects**: a workspace on an opportunity where the work is scoped before it's quoted. It produces three linked artifacts, each drafted by AI (with the customer's own provider, §8.8) and edited and approved by people. They then assemble the quote.
  - **Discovery:** what the scoping draws on.
    - meeting notes, call transcripts and uploaded documents (RFPs, site surveys)
    - the client's environment from the CMDB (users, sites, devices, licences, lifecycle)
    - the client's agreements and history
    - similar past opportunities and projects (by meaning, §8.8)
  - **Scope of Work (SOW) builder:**
    - Sections from a template per kind of engagement: objectives, in scope, out of scope, deliverables, assumptions, client responsibilities, acceptance criteria, timeline, change control.
    - AI drafts each section from discovery and past SOWs, citing what it drew on. It never invents facts: gaps become open questions for the sales engineer.
    - Versioned, with a branded document that is attached to the quote and signed with it.
  - **Work Breakdown Structure (WBS) generator:**
    - Phases, tasks, milestones and dependencies, each task with hours by role and a low/likely/high estimate, drafted from the SOW's deliverables.
    - Grounded in actuals, not guesses: project templates (§8.14), and the hours logged against similar past projects' tasks.
    - Contingency is set by risk.
    - Every SOW deliverable must map to WBS tasks, and gaps are flagged.
  - **Bill of Materials (BOM) generator:**
    - Hardware, software, licences and third-party services, drafted from the SOW and the client's environment. For example, 45 users and 3 sites become licences, firewalls and access points.
    - Quantities follow drivers (users, sites, devices), so changing one updates the lines.
    - Lines are matched to the product catalog, with distributor pricing and availability (CPQ), alternates, and warnings for end-of-life or incompatible items.
  - **Quote assembly:**
    - WBS hours × the rate card become labour lines, priced per the billing method (fixed fee, by milestone, T&M, not-to-exceed, §8.14).
    - BOM lines become product lines under the margin rules, with recurring services alongside.
    - Consistency checks run before sending: every deliverable is costed, and every BOM item is referenced in the scope.
  - **On a win:** the WBS becomes the project (phases, tasks, estimates, budget, billing method; §8.14), and the BOM feeds the sales order and purchase orders.
  - **Learning:** when the project closes, estimated hours are compared with actuals by task and role, and the comparison feeds the next WBS.

### 8.11 Invoice generation and accounting sync (later phase, designed now)

Huxley **generates** invoices from the work and sales it tracks, then **syncs** them to the MSP's accounting system through integrations. Huxley is the source of truth for *what to bill*. The accounting system is the system of record for posted invoices, payments, AR, and tax filing. Huxley does not process payments itself.

- **Invoice sources**:
  - Billable time entries. Agreement-aware: each entry is classified as covered or billable. Optionally only time on approved timesheets (§8.15).
  - Expenses.
  - Project billing (§8.14): fixed fee, milestone, T&M, not-to-exceed.
  - Product and sales order lines.
  - Agreement recurring charges, with proration and usage-based quantities from datasets (e.g., M365 seat counts).
- **Billing runs**: select period/companies → system generates invoice drafts → review (line edits, holds, notes, PDF preview) → approve → sync. Generated sources are marked billed so nothing is billed twice. Unbilled work in progress is always visible.
- **Internal invoice approval before anything leaves Huxley.** No invoice is sent to a client, or synced to the accounting system or any other integration, until it's approved. It uses the same review model as quotes (§8.10):
  - customizable rules (total, change from last period, credit memos, new clients, lines edited by hand, specific clients)
  - the approval as an editable workflow
  - review threads with line comments and `@mentions`
  - approve, request changes or reject, with a diff on resubmission
  - an audited override
  - Whole billing runs can be approved in one pass, with the exceptions flagged.
- **Accounting sync**: built-in QuickBooks Online and Xero packs plus others via integrations. The sync itself is a system workflow, so MSPs can adapt it or add other accounting platforms. It has three parts:
  - A mapping UI: companies ↔ customers, products/work types ↔ items, tax codes, classes/tracking categories, currencies.
  - Sync status per invoice.
  - An error queue with retry.
- **Status back-sync**: payment status, balance, and overdue state flow back from the accounting system onto the company record. They are used by the client portal, client health (§8.9), and optional credit-hold events (`company.credit_hold`).
- **Tax**: Huxley applies tax codes per line for preview, and the accounting system's calculation is authoritative. Multi-currency is in the model but optional.
- **Corrections**: after an invoice syncs it is locked in Huxley. Adjustments are made as credit memos generated and synced the same way.
- **Data model rule**: monetary values use `numeric(18,4)` with an explicit currency.

### 8.12 Analytics, reporting and dashboards

- **Semantic data layer**: a curated, permission-aware catalog of queryable *data sources*. These include core entities (tickets, time entries, assets, invoices…), with fields, joins, and measures defined once, plus **datasets** (§9). Users never write raw SQL by default. Queries compile from a structured query spec to parameterized SQL, with RLS and RBAC applied.
- **Widget builder**: choose a data source → fields, filters, group-by, measures → visualization (number/KPI, table, bar, line, area, pie, heatmap, gauge, list, map). Supports drill-through to records.
- **Dashboard builder**: grid layout, global filters (date range, company, queue), per-user, per-team, and per-org dashboards, and live refresh.
- **Reports**: tabular or paginated, exportable (CSV, XLSX, PDF), scheduled delivery via a system workflow.
- **Publishing workflow**: personal → *submit for publishing* → reviewer approves → published to selected roles, teams, orgs, or the client portal. Published items are versioned. Consumers can clone them.
- Heavy queries can be routed to a **read replica**. Results are cached with TTL. Query cost limits and timeouts apply.

### 8.13 Client portal

- A separate SvelteKit app (`portal`), white-labeled per provider org (logo, colors, domain/subdomain), and optionally per client.
- **Portal users** are contacts with portal access. Auth: local (magic link and/or password + MFA), or **OIDC with the client's own IdP** (e.g., the client's Entra tenant, matched by domain).
- Capabilities, all gated by **portal RBAC** (§11):
  - Tickets: view, create, comment, approve, CSAT.
  - Service catalog: forms that trigger workflows.
  - Assets, KB, published dashboards/widgets.
  - Quotes (accept), invoices (view, with links to the accounting system's view/pay page when available), project status, change approvals.
  - Company-admin contacts can manage their colleagues' access.

### 8.14 Project management

- **Projects** belong to a client company. They can be linked to an opportunity/sales order, an agreement, and a project manager. Each project has a type, status workflow, start/end dates, and a health indicator (on track / at risk / off track, with reasons).
- **Structure**: phases → tasks → checklists, plus milestones. Tasks have finish-to-start dependencies.
- **Project tasks are tickets** (ticket type *Project Task*) in a project queue. Technicians see them in **My Work** next to service tickets and log time the same way. Notes, attachments, SLAs (optional), and automation work identically.
- **Templates**: reusable project templates (e.g., *New client onboarding*, *M365 migration*, *Server refresh*) with phases, tasks, default estimates, and attached workflows. A system workflow can create a project from a template when a sales order is approved (§8.10).
- **Views**: list, board (by status or phase), timeline/Gantt with dependencies and the critical path, and a calendar.
- **Resource planning**: estimated hours per task and role. A capacity view per technician/team is shared with the dispatch calendar (§8.2), with over-allocation warnings.
- **Budgets and actuals**: hour and cost budgets per project and phase, compared against time entries, expenses, and allocated products. Threshold events (`project.budget_threshold`) fire at configurable percentages.
- **Billing methods**: fixed fee (optionally split by milestone), T&M, not-to-exceed, or covered by an agreement. These feed invoice generation (§8.11).
- **Scope change requests**: change requests with client approval (portal/email via `approval` steps) adjust budget and schedule. These are separate from ITIL changes (§8.6) but can link to them.
- **Client visibility**: optional portal project view with milestones, status updates, and client-facing tasks.
- **Status reports**: scheduled project status summaries, AI-drafted from task progress and notes, sent to clients via workflow.
- **Events**: `project.created`, `project.status_changed`, `project.milestone_reached`, `project.task_completed`, `project.budget_threshold`, `project.overdue`.

---

### 8.15 Workforce: skills, certifications, availability and timesheets

The `workforce` module describes the MSP's people: what they can do, when they're available, and the time they work. Dispatch, project resource planning and billing all use it.

- **Skills:**
  - A skills catalog, in categories such as *Microsoft 365*, *Networking* or *Security*. Each skill is rated on levels: *learning*, *working*, *proficient*, *expert*.
  - Each person has skills at a level. A level is either self-assessed or verified by a lead, with who verified it and when.
  - **Requirements:** queues, ticket types and ticket templates can require skills, with a minimum level. AI triage (§8.8) can also suggest the skills a ticket needs.
  - A **skills matrix** view shows people against skills, with gaps highlighted per queue or team.
- **Certifications:**
  - A catalog of certifications, each with a vendor, name, code, and validity period, such as Microsoft *Azure Administrator* or Cisco *CCNA*.
  - What a person holds: when earned, when it expires, the credential ID and verification link, and an evidence attachment.
  - **Expiry events:** `certification.expiring` (at configurable lead times) and `certification.expired` feed a default reminder workflow, to the person and their lead.
  - **Hard requirements:** a queue or ticket type can require a certification. Dispatch then only assigns its work to someone holding it, unexpired. This suits regulated clients, or vendors that require certified engineers.
  - **Partner-program coverage:** for example, how many people hold each certification a vendor partner program counts, and when that will change.
- **Availability:**
  - **Working schedule:** each person has a working schedule (hours per weekday, timezone), which defaults from their team.
  - **Time off:** requested and approved through the approval step.
  - **Calendar sync:** people's work calendars, so Huxley knows when they're actually busy:
    - **Sources:**
      - Microsoft 365 (Graph) and Google Workspace. An admin consents once for the tenant or domain and picks who's synced; a person can also connect their own calendar.
      - A read-only ICS feed URL for anything else.
    - **Reading:**
      - Free/busy blocks (start, end, busy, tentative or out of office) are kept per person, a few weeks back and ahead.
      - Changes arrive by push (Graph change notifications, Google push channels), with periodic delta sync as the fallback and to catch missed notifications.
      - Private events are stored as *Busy* only. Titles are kept only where the admin allows it, and are shown only to the person and dispatchers.
    - **Writing:** dispatched appointments and scheduled project work are written to the person's calendar (an event Huxley owns, linked to the ticket), and updated or removed when the work moves, is reassigned or is cancelled. Writing is opt-in per connection.
    - **Conflicts:** an appointment overlapping a busy block is flagged when it's booked. A calendar change that newly conflicts with booked work raises `calendar.conflict`, which a workflow can route to the dispatcher.
    - **Out of office** in the calendar counts as away, alongside time off approved in Huxley.
  - **Uses:**
    - Dispatch skips people who are away, outside their hours, or busy in their calendar, and only offers slots that are actually free.
    - Capacity planning (§8.14) uses expected hours, less calendar time that's busy.
    - Timesheets compare logged time against expected time.
- **Time entries:**
  - Logged on tickets and project tasks (§8.2, §8.14), or as non-ticket time with a work type: *admin*, *internal meeting*, *training*, *travel* or *time off*.
  - Each entry is billable or not, has a work type and notes, and can be linked to an agreement.
  - Technicians can use a timer, and AI can draft entries from their ticket activity for them to confirm.
- **Timesheets:**
  - A timesheet covers a period, weekly by default and configurable.
  - **Flow:** the person submits it, their lead (team lead, manager, or a configured approver) approves it or rejects it with comments, and approved timesheets lock their entries.
  - **Corrections:** reopening an approved timesheet is audited and goes back through review.
  - **Reminders** are system workflows, so their rules can be changed. They go out for:
    - unsubmitted or late timesheets
    - days logged under expected hours
    - entries missing notes
  - **Uses:** billing runs can use only approved time (§8.11). A payroll export (CSV, or an integration pack) uses approved time and time off.
- **Analysis** (dashboards in §8.12):
  - utilization: billable over available hours
  - billable percentage
  - hours by client, agreement, work type and queue
  - overtime and under-logging
  - estimate against actual on tickets and project tasks
  - trends per person and team
- **Permissions:**
  - `workforce:read`
  - `workforce:manage`: catalogs, people's skills and certifications.
  - `timesheet:submit`: one's own.
  - `timesheet:approve`, scoped to the people one leads.

### 8.16 Expenses

The `expenses` module tracks money people spend. It covers the MSP's own costs, reimbursing staff, and expenses billed to clients through the ticket or project they were for.

- **Expense entries:**
  - date, amount and currency, a category (mileage, travel, meals, parts, software, subcontractor or custom), the vendor, notes and receipts (photos or PDFs, with AI reading the amount, date and vendor for the person to confirm, §8.8)
  - billable or not; reimbursable or company-paid
  - tax
- **Attached to work:** an expense can belong to a ticket, a project (a phase or task, §8.14) or a client, or be internal. On a ticket or project it shows beside the time logged.
  - Billable expenses carry a markup (by category or agreement), and are evaluated against agreement coverage like time.
  - They count against project budgets: cost budgets, and burn.
- **Mileage:** distance × a configurable rate, with a start and end, or a route.
- **Reports and approval:**
  - People group expenses into reports, which go to their lead (or a configured approver) for approval through the approval step. Approval chains are workflows.
  - Approve, reject or request changes, with comments.
  - Approved reports lock their entries.
  - Policies flag what's over a limit, missing a receipt or out of policy.
- **Reimbursement:** approved reimbursable expenses are marked paid when reimbursed, with an export, or synced to the accounting system as bills or expense claims through the accounting integrations (§8.11).
- **Billing:** approved billable expenses are invoice sources (§8.11). They're marked billed once invoiced, so nothing is billed twice.
- **Cards and imports:** company card transactions can be imported (CSV, or through integrations) and matched to entries.
- **Events** (`expense.submitted`, `expense.approved`, `expense_report.approved`, `expense.reimbursed`) let workflows automate the rest.

## 9. Custom Datasets

Datasets let users pull data from integrations into Huxley so it can be queried, reported on, joined, and used in workflows and forms without calling external APIs every time.

- **Definition**:
  - Source: an integration list action, a workflow output, or a CSV upload.
  - Schema: typed columns, inferred from the action's output schema and editable.
  - Primary key, and sync mode: full refresh, incremental (watermark), or upsert-by-key.
  - Schedule, retention, and per-org scoping. A dataset defined at a parent can sync per descendant org using each org's connection, e.g., "M365 users for every client".
- **Storage**: each dataset gets a **managed, typed table** in a dedicated `datasets` schema (generated DDL, with migrations when the schema changes) plus `org_id` and sync metadata. Typed tables keep queries fast and indexable. JSON-ish columns use `jsonb`.
- **Sync**: implemented as a system workflow (`dataset.sync`) using integration pagination and rate limits, so syncs are visible, retryable, and observable like any run.
- **Query**: through the semantic layer (§8.12), the `dataset.query` workflow step, form dynamic options, the Jinja `lookup()` helper, the API, and MCP. An optional **read-only SQL console** for admins runs as a restricted Postgres role limited to the `datasets` schema with RLS, a statement timeout, and row limits.
- **Mappings**: datasets can map into core entities (assets, contacts, licenses) via match and merge rules (§8.5).

---

## 10. AI Assistant and MCP

### 10.1 AI provider layer

- A `huxley-ai` crate with a `ModelProvider` trait covering chat, tool calls, structured output, embeddings, and streaming. Implementations: Anthropic, OpenAI, Azure OpenAI, and generic OpenAI-compatible.
- Keys are stored as encrypted connections and inherited down the org tree. Model selection is set per feature.

### 10.2 MCP server

- **Transport**: Streamable HTTP at `/mcp` (plus stdio via `huxley-cli mcp` for local clients).
- **Auth**: OAuth 2.1 (Huxley as the authorization server, per the MCP auth spec) or a personal access token. **All tool calls run as the authenticated user** and pass through the same RBAC as the API.
- **Tool groups** (each can be toggled per org/role):
  - *Automation*: integrations/actions catalog, workflow CRUD (drafts), validate, test-run, runs/logs, scripts, forms, variables (non-secret).
  - *Service desk*: search/get/create/update tickets, add notes, time entries.
  - *Clients and assets*: company/contact/asset lookups, dataset queries.
  - *Actions*: published workflows flagged `mcp_tool`, exposed as first-class tools with their input schemas.
- **Resources**: KB articles, workflow definitions, the HWF JSON Schema, integration action docs.
- Destructive tools need explicit confirmation semantics: they require a `confirm` parameter, or produce a draft that a human approves.

### 10.3 Built-in assistant

A chat panel in `web` (and optionally `portal`, with portal-scoped tools). It uses the same MCP tool layer internally, keeps conversation history per user (retention configurable), streams responses over SSE, and shows tool calls transparently with approve/deny prompts for side-effecting actions.

---

## 11. Identity, Authentication and Authorization

### 11.1 Identities

| Identity | Description |
|---|---|
| **Staff user** | MSP technicians and admins. Belongs to the instance. Has role assignments scoped to orgs. |
| **Portal user** | Linked to a client contact. Lives in a separate auth realm (separate session cookie and domain/path) and is limited to the portal API surface. |
| **Service account** | Non-human, for integrations and the CLI. Uses API tokens with scopes and expiry. |
| **Workflow identity** | Runs execute as either the *invoking user* (manual, form, MCP) or the workflow's configured *run-as service account* (schedule, webhook, event). Core actions check permissions against that identity. |

### 11.2 Authentication

- **Local accounts**: Argon2id hashing, breached-password check (k-anonymity, optional), lockout and backoff, email verification, password reset with single-use tokens.
- **MFA**: TOTP and WebAuthn/passkeys, enforceable per role or org. Passkey-only login supported.
- **OIDC SSO**: any OIDC provider, with first-class presets for **Microsoft Entra ID** (multi-tenant or single-tenant, group/app-role claim mapping to Huxley roles), Google, Okta, Authentik, and Keycloak. JIT provisioning with claim → role/team mapping. **SCIM 2.0** provisioning comes later.
- **Sessions**: server-side sessions stored in Postgres and cached in Valkey. `HttpOnly`, `Secure`, `SameSite=Lax` cookies, rotation on privilege change, idle and absolute timeouts, a device/session list with revoke.
- **API tokens / PATs**: hashed at rest, scoped (subset of the user's permissions), expiring, last-used tracking.

### 11.3 Authorization (RBAC + scoped ABAC)

**Model**:

- **Permissions** are fine-grained strings, `resource:action[:qualifier]`. Examples: `ticket:read`, `ticket:update`, `ticket:update:status`, `ticket:delete`, `ticket.time_entry:create`, `ticket.note.internal:read`, `workflow:publish`, `connection:use`, `connection:manage`, `secret:reveal`, `invoice:post`, `report:publish`.
- **Roles** bundle permissions. Huxley ships defaults (Owner, Admin, Service Desk Manager, Technician L1/L2, Sales, Finance, Read-only, Automation Engineer; portal defaults: Client Admin, Client User, Client Approver). Custom roles are allowed.
- **Role assignments** = `(principal: user|team|service account, role, scope: org node, inherit_to_descendants: bool)`.
- **Conditions** (ABAC) attach to assignments or policies, e.g., "only tickets in queues the user is a member of", "only own time entries", "only assets with tag X", "only during business hours" (rare).
- **Field-level permissions**: sensitive fields (cost price, margin, salary-ish rates, credentials, internal notes) need explicit permissions. The API strips them from responses, and forms and widgets respect them.
- **Record-level**: org scope + conditions → enforced by query filters (and RLS for the org scope).
- **Portal RBAC**: the same engine with portal-specific roles, scoped to the contact's company (and optionally sub-orgs/sites). Rules like "can see all company tickets" vs "only own tickets".

**Engine**: assignments and conditions compile to **Cedar policies**, evaluated in-process by the `Authorizer` trait. Two kinds of check are supported:

1. Point checks: `can(principal, action, resource)`.
2. **Query filters**: compiled to SQL predicates for list endpoints, so pagination stays correct.

Decisions are cached per request.

**Implementation sequencing:** the first RBAC slice (Phase 1) evaluates role × org-scope grants with inheritance in-process, behind `Principal::can_at_path`, and derives the RLS scope from the same grants. Cedar takes over evaluation behind that interface when attribute conditions (ABAC) and field-level rules arrive, so handlers don't change. An admin **"effective permissions" / "why can/can't this user…"** explainer helps debugging.

---

## 12. Security Design

| Area | Design |
|---|---|
| **Secrets at rest** | Envelope encryption: each secret (connection credential, secret variable, vault item) is encrypted with AES-256-GCM using a per-provider-org data key (DEK), so each MSP brand under a common owner has separate keys. DEKs are encrypted by a key-encryption key (KEK) from a KMS provider: env/file (default), **Azure Key Vault**, HashiCorp Vault, or DO-compatible KMS. Supports key rotation and re-wrap. |
| **Secrets in use** | Decrypted only in the worker or API process at use time. Never returned by the API (write-only fields). Revealing requires `secret:reveal` and is audited. Redacted from logs, run history, AI prompts, and errors. |
| **Transport** | TLS everywhere (reverse proxy or native rustls). HSTS. mTLS between workers and runners. |
| **Web app** | Strict CSP with nonces, CSRF protection (double-submit token + SameSite), output encoding (Svelte default), sanitized rich text (ammonia on the server), clickjacking protection, secure headers. |
| **SSRF / egress** | The HTTP node, integrations, and runners go through an **egress policy**: block private/link-local/metadata IP ranges by default (admin can allowlist), DNS rebinding protection (resolve-then-pin), protocol allowlist, size and time limits. |
| **Webhooks** | Unguessable IDs, signature verification, replay protection, per-endpoint rate limits, payload size limits. |
| **Rate limiting** | Per IP, user, token, and endpoint class, especially on auth endpoints. |
| **Audit log** | Append-only `audit_events` (who, what, when, from where, via UI/API/MCP/workflow, before/after diff for config changes). Hash-chained for tamper evidence. Exportable to a SIEM via syslog/webhook/stream. |
| **Input validation** | Strongly typed request DTOs (serde + validator). JSON Schema validation for dynamic inputs. Max sizes everywhere. |
| **Supply chain** | `cargo-deny` (licenses: MIT-compatible only; advisories), `cargo-audit`, `pnpm audit`, Grype image scanning, GitHub Actions pinned to commit SHAs, pnpm minimum release age and blocked install scripts, Renovate, reproducible builds, SBOM (CycloneDX), cosign-signed images, signed packs. |
| **Pack trust** | Packs are signed. Importing an unsigned or third-party pack shows a full manifest of what it adds (integrations, egress domains, scripts) and requires admin approval. |
| **Data protection** | Per-org data retention policies, export and erase tooling for contacts (GDPR), encrypted backups guidance. |
| **Security process** | `SECURITY.md` with a private disclosure policy, threat model kept in repo (STRIDE per component), periodic external review once mature. Target: **OWASP ASVS Level 2**. |

---

## 13. Reliability, Scalability and Operations

### 13.1 Reliability

- Stateless API with health endpoints (`/healthz` liveness, `/readyz` checking DB/Valkey).
- Graceful shutdown: stop claiming jobs, finish or release in-flight work, drain WebSockets.
- Durable engine (§6.4): transactional outbox, heartbeats, reaper, idempotency keys, dead-letter view with bulk retry.
- Database migrations are **expand → migrate → contract**, so rolling deploys work without downtime. Migrations run via `huxley migrate` as an init job.
- Backups: Postgres PITR (managed DB on DO/Azure, or WAL-G/pgBackRest for self-run) plus object storage versioning. Documented restore drills.

### 13.2 Scaling

| Tier | Scaling approach |
|---|---|
| API | Horizontal (stateless). Sticky sessions not required (WebSocket fan-out via pub/sub). |
| Workers | Horizontal, autoscaled on queue depth/latency (KEDA on k8s; CPU-based on Container Apps/App Platform). Workers can be pooled by job class (e.g., `email`, `ai`, `default`) to isolate workloads. |
| Scheduler | Single active leader (Postgres advisory lock) with hot standby. Its work is lightweight (enqueue only). |
| Runners | Horizontal pool per language, autoscaled. |
| PostgreSQL | Vertical first. Read replicas for reporting and analytics. Partitioning for runs, step runs, audit events, and email. PgBouncer for connection pooling. |
| Valkey | Single instance → replica/sentinel or managed (Azure Cache, DO Managed Caching). |

### 13.3 Observability (OpenTelemetry)

**OpenTelemetry is the single instrumentation standard** across every Huxley component. Nothing is vendor-specific. Operators point OTLP at whatever backend they run: Grafana LGTM, Jaeger, Azure Monitor / Application Insights, Datadog, Honeycomb, etc.

- **Signals**: traces, metrics, and logs, all exported via OTLP (gRPC or HTTP).
  - Rust uses `opentelemetry`, `opentelemetry-otlp`, and `tracing-opentelemetry`.
  - Logs are bridged from `tracing`, so every log line carries trace and span IDs. Logs also go to stdout as structured JSON for setups without a collector.
  - An optional `/metrics` Prometheus scrape endpoint (OTel Prometheus exporter) supports setups that don't run a collector.
- **What is instrumented**:
  - All Rust roles: api, worker, scheduler, runner.
  - The SvelteKit servers via `@opentelemetry/sdk-node`. The browser SDK is optional and off by default.
  - Auto-instrumented spans for DB queries (sqlx), Valkey, outbound HTTP, and gRPC.
- **Workflow runs as traces**:
  - Each run is a span, linked to the trace that triggered it (e.g., webhook request → run, or ticket update → event → run). Each step attempt is a child span.
  - Attributes: `huxley.workflow.key`, `huxley.workflow.version`, `huxley.run.id`, `huxley.step.id`, `huxley.step.type`, `huxley.step.attempt`, `huxley.org.id`, `huxley.integration`, outcome.
  - Long waits (`wait`, callbacks, approvals) close the span when the run parks. The resume starts a new span connected by a **span link**, so multi-day runs never hold spans open.
- **Context propagation** (W3C `traceparent`/`tracestate`):
  - API → job queue (trace context stored on the job row) → worker → runner (gRPC metadata, exposed to scripts as `TRACEPARENT`) → outbound integration calls.
  - Outbound propagation is configurable per integration, for APIs that reject unknown headers.
  - Inbound webhooks can optionally continue a caller's trace.
- **Metrics**: OTel semantic conventions where they exist (HTTP, DB, RPC), plus `huxley.*` metrics:
  - queue depth and latency, step duration, runs by status
  - integration requests, errors, and rate-limit hits per integration
  - SLA warnings and breaches, mail ingestion lag
  - AI tokens per provider and feature, DB pool usage

  Exemplars link metric spikes to example traces.
- **Privacy and cardinality**: no payloads, PII, or secrets in telemetry. The same redaction layer that protects run history scrubs span attributes and log fields. Per-org attributes can be turned off on large installs to control cardinality.
- **Sampling**: parent-based with a configurable ratio. The shipped collector config uses tail sampling that keeps all errors and slow runs.
- **Deployment**:
  - Compose includes an optional `otel-collector` profile with example exporters.
  - Azure templates wire OTLP to Azure Monitor.
  - Helm values expose OTLP endpoint and sampling settings.
  - Grafana dashboards and alert rules ship in `deploy/observability/`.
- **Two audiences**: in-app run history (stored in Postgres) remains the *user-facing* view of what workflows did. OpenTelemetry is the *operator-facing* view of how the platform behaves. Run detail pages show the trace ID, plus an optional "open in tracing backend" link built from a configurable URL template.
- An in-app **System Health** page for admins: workers, runners, queue, failed runs, connection health, mailbox lag.

### 13.4 Deployment targets

| Target | Shape |
|---|---|
| **Docker Compose** (reference) | `huxley` (roles=all), `web`, `portal`, `runner-pwsh`, `runner-python`, `postgres`, `valkey` (optional), `caddy` (TLS), optional `otel-collector` profile. One-command install script with generated secrets. |
| **Kubernetes / Helm** | Separate deployments per role, HPA/KEDA, external Postgres/Valkey, Ingress, cert-manager. |
| **DigitalOcean** | Droplet + Compose (small), or App Platform / DOKS + Managed PostgreSQL + Spaces. Terraform template. 1-Click/Marketplace image later. |
| **Azure** | Azure Container Apps (recommended) or AKS + Azure Database for PostgreSQL Flexible Server + Blob Storage + Key Vault (KEK) + Entra (OIDC). Terraform template. |
| **AWS** | ECS Fargate (recommended) or EKS + RDS for PostgreSQL + S3 + KMS (KEK) + Secrets Manager + ALB, optionally CloudFront. Terraform template. |

Configuration uses 12-factor environment variables plus an optional `huxley.toml`. All secrets accept `*_FILE` variants for Docker and k8s secrets.

---

## 14. Data Model Overview

High-level groups. The full schema is defined per module in migrations.

```
platform:   organizations(path ltree, kind), org_settings, users, user_identities(oidc),
            sessions, api_tokens, teams, team_members, roles, role_permissions,
            role_assignments, policies, audit_events(partitioned), notifications,
            custom_field_defs, custom_field_values(jsonb on entities), tags, attachments,
            events_outbox, kms_keys

automation: integrations, integration_versions(spec+overlay), connections(enc),
            org_variables(enc for secret), scripts, script_versions, workflows,
            workflow_versions(hwf jsonb), triggers, webhooks, schedules, forms,
            form_versions, form_submissions, runs(partitioned), step_runs(partitioned),
            jobs, timers, callbacks, packs, pack_installs, datasets, dataset_syncs,
            git_sync_configs

crm/psa:    sites, contacts, contact_companies, queues, queue_members, ticket_types,
            status_workflows, tickets, ticket_notes, ticket_links, time_entries,
            sla_policies, business_calendars, sla_clocks, mailboxes, inbound_emails,
            assets(ci), ci_types, ci_relationships, kb_articles, kb_versions,
            changes, change_approvals, change_windows, agreements, agreement_lines,
            warranties, products, bundles, warehouses, stock_levels, stock_movements,
            serials

projects:   projects, project_templates, project_phases, project_milestones,
            project_task_deps (tasks are tickets), project_budgets,
            project_change_requests, resource_allocations, project_status_reports

cx/ai:      sentiment_scores, csat_surveys, csat_responses, nps_campaigns,
            nps_responses, health_scores, ai_providers, ai_calls, embeddings(pgvector),
            assistant_conversations

sales:      leads, opportunities, pipeline_stages, activities, communications,
            quotes, quote_versions, quote_lines, sales_orders, purchase_orders,
            po_lines, receipts, forecasts

billing:    billing_runs, invoices, invoice_lines, credit_memos, billed_sources,
            tax_codes, accounting_mappings, accounting_sync_records,
            payment_status (synced from accounting)

reporting:  data_sources(semantic), widgets, dashboards, reports, publications,
            report_schedules
```

Conventions:

- UUIDv7 primary keys (sortable).
- `org_id` on all org-owned data.
- `created_at/by`, `updated_at/by`.
- Soft delete (`deleted_at`) where users expect restore.
- Optimistic concurrency via a `version` column.
- Human-friendly sequential numbers for tickets, projects, quotes, invoices, and POs. Sequences are instance-wide, with an optional per-provider-org prefix (e.g., `ACME-1042`).

---

## 15. Codebase Structure

A **single monorepo** for the platform, so API, engine, UI, and schema changes land atomically, plus a few satellite repos.

```
huxley/                              # main monorepo (MIT)
├── Cargo.toml                       # Rust workspace
├── crates/
│   ├── huxley-server/               # binary: roles, wiring, config
│   ├── huxley-cli/                  # binary: dev CLI (workflows-as-code, mcp stdio)
│   ├── huxley-runner/               # binary: script runner host (gRPC)
│   ├── huxley-core/                 # shared domain types, errors, ids, events
│   ├── huxley-db/                   # pool, tx helpers, RLS context, migrations
│   ├── huxley-telemetry/            # OpenTelemetry setup: OTLP traces, metrics, logs
│   ├── huxley-api/                  # axum routers, DTOs, OpenAPI (utoipa)
│   ├── huxley-auth/                 # local, OIDC, MFA, sessions, tokens
│   ├── huxley-authz/                # Cedar-backed Authorizer, policy compiler
│   ├── huxley-crypto/               # envelope encryption, KMS providers
│   ├── huxley-orgs/                 # org tree, inheritance resolution
│   ├── huxley-engine/               # executor, queue, scheduler, triggers
│   ├── huxley-hwf/                  # workflow format: types, schema, validation
│   ├── huxley-expr/                 # minijinja env, filters, taint tracking
│   ├── huxley-integrations/         # OpenAPI import, overlays, HTTP executor, OAuth
│   ├── huxley-datasets/             # dataset DDL mgmt, sync, query compiler
│   ├── huxley-ai/                   # provider abstraction, embeddings
│   ├── huxley-mcp/                  # MCP server
│   ├── huxley-mail/                 # Graph/IMAP ingestion, parsing, SMTP
│   ├── huxley-modules-crm/          # companies, contacts, sites
│   ├── huxley-modules-servicedesk/  # tickets, queues, SLA
│   ├── huxley-modules-cmdb/         # assets, relationships, KB
│   ├── huxley-modules-change/
│   ├── huxley-modules-projects/     # projects, templates, resource planning
│   ├── huxley-modules-contracts/    # agreements, warranties
│   ├── huxley-modules-inventory/
│   ├── huxley-modules-cx/           # csat, nps, health
│   ├── huxley-modules-sales/        # pipeline, cpq, orders, POs
│   ├── huxley-modules-billing/      # invoice generation, accounting sync
│   └── huxley-modules-reporting/    # semantic layer, widgets, dashboards
├── migrations/                      # sqlx migrations (ordered, per module prefix)
├── apps/
│   ├── web/                         # SvelteKit staff app
│   └── portal/                      # SvelteKit client portal
├── packages/                        # pnpm workspace
│   ├── ui/                          # design system (Svelte components, tokens)
│   ├── api-client/                  # generated TS client from OpenAPI
│   ├── canvas/                      # SvelteFlow workflow canvas components
│   ├── form-renderer/               # JSON-Schema form renderer (shared web/portal)
│   ├── widgets/                     # chart/widget renderer (shared web/portal)
│   └── sdk/                         # @usehuxley/sdk: TypeScript workflows-as-code → HWF
├── packs/                           # built-in packs (system workflows, integrations)
├── runners/                         # Dockerfiles for pwsh/python/js runners
├── deploy/
│   ├── compose/                     # docker-compose + install script
│   ├── helm/huxley/
│   ├── azure/                       # bicep/terraform
│   └── digitalocean/                # app spec / terraform
├── docs/                            # architecture, ADRs, threat model
└── .github/workflows/
```

**Satellite repositories:**

- `huxley-packs`: community integration/workflow packs and the registry index.
- `huxley-docs`: the documentation website at docs.usehuxley.io (could live in `docs/` initially).

The TypeScript SDK lives in the monorepo (`packages/sdk`), so it changes in lockstep with the HWF schema.

**Public names:**

| Thing | Name |
|---|---|
| Product site | `usehuxley.io` |
| Documentation | `docs.usehuxley.io` |
| Pack registry | `packs.usehuxley.io` (index backed by the `huxley-packs` repo) |
| Published schemas | `https://usehuxley.io/schemas/hwf/v1.json` (plus forms and packs) |
| HWF `apiVersion` | `usehuxley.io/v1` |
| npm scope | `@usehuxley` (`@usehuxley/sdk`, `@usehuxley/api-client`) |
| Container images | `ghcr.io/usehuxley/huxley`, `…/web`, `…/portal`, `…/runner-pwsh`, `…/runner-python`, `…/runner-js`. Published from CI in the `usehuxley` GitHub org. |
| GitHub org | `github.com/usehuxley` (created) |
| Security contact | `security@usehuxley.io` (in `SECURITY.md`) |

---

## 16. UX Principles

- **Command palette** (`Ctrl/Cmd+K`) everywhere: jump to any ticket, client, asset, or workflow, or run any Action.
- **Global search** with typed results and keyboard-first navigation.
- **Context everywhere**: on a ticket, the client 360 side panel, related assets, recent runs, suggested KB articles, and suggested Actions are one click away.
- **Consistent list/detail pattern**: saved views, inline edit, bulk actions, keyboard shortcuts, density toggle.
- **Real-time** collaboration cues (presence, live updates) without page refreshes.
- **Light/dark themes**, accessible (WCAG 2.2 AA), responsive layouts, localization-ready (i18n keys from day 1).
- **Empty states that teach**: every module explains what to do next and offers templates.
- The design system lives in `packages/ui` and is shared by web and portal. The portal is themable.

---

## 17. Key Decisions (ADRs to record)

| # | Decision | Rationale |
|---|---|---|
| ADR-001 | Rust monolith binary with role subcommands | Simple small installs, independent scaling at size. Avoids microservice overhead. |
| ADR-002 | Postgres-backed job queue (Valkey optional) | Transactional enqueue, fewer moving parts, proven pattern. Can swap in a broker later behind a trait. |
| ADR-003 | Journal-based durable execution (not deterministic replay) | Easier to reason about for users and for a visual tool. Every step's I/O is inspectable. |
| ADR-004 | HWF as single canonical workflow format | Lets canvas, code, and MCP coexist with round-tripping. |
| ADR-005 | minijinja for expressions | Jinja requirement, Rust-native, sandboxed. |
| ADR-006 | Cedar for authorization evaluation | Granular ABAC, formally analyzable, Rust-native, Apache-2.0. |
| ADR-007 | Scripts run only in isolated runners | Security boundary; PowerShell is a must for MSPs. |
| ADR-008 | Datasets as generated typed tables | Query performance, indexing, works with the semantic layer. |
| ADR-009 | Separate SvelteKit apps for staff and portal | Smaller attack surface, independent theming and auth realms, shared packages. |
| ADR-010 | Single-owner instances; multiple MSPs modeled as provider orgs in the org tree; RLS on org scope | Matches the intended use (one company, possibly owning several MSPs) without a tenant layer. RLS is defense in depth. |
| ADR-011 | System automations ship as packs of workflows | "Engine as the platform": everything inspectable and customizable. |
| ADR-012 | MIT-compatible dependencies only (enforced by cargo-deny/license checker) | Licensing integrity. |
| ADR-013 | On-prem execution via integrations (RMM, Intune, Arc); no first-party agent for now | Uses tooling MSPs already run; keeps runner protocol open for a future agent. |
| ADR-014 | Huxley generates invoices; accounting systems are the system of record | Avoids rebuilding AR, payments, and tax compliance; syncs via integration packs. |
| ADR-015 | OpenTelemetry as the only instrumentation standard (OTLP export) | Vendor-neutral; works with any backend operators already run, including Azure Monitor. |
| ADR-016 | TypeScript SDK compiles to HWF at build time; no SDK runtime | One execution path, one security model; code and canvas stay interchangeable. |
| ADR-017 | Everything outside the core platform is a toggleable feature module | Lets MSPs run Huxley as an automation engine only; one registry gates UI, API, permissions, and system workflows. Data is kept when a module is off. |

---

## 18. Decisions and Open Questions

### 18.1 Resolved (2026-11-21)

| Question | Decision | Where reflected |
|---|---|---|
| Multi-tenancy | One instance serves one owning company. Multiple MSPs are supported only when a parent company owns them, modeled as provider orgs in the org tree. Not sold as multi-tenant SaaS. | §4.4, §5.1, ADR-010 |
| On-prem script execution | Through integrations (RMM script execution, Intune, Azure Arc). A Huxley agent is a future possibility only. | §6.9, §7.3, ADR-013 |
| Billing scope | Invoice generation in Huxley, synced to QuickBooks Online, Xero, etc. via integrations. No payments or AR in Huxley. | §8.11, ADR-014 |
| Project management | Included. Tasks are tickets, with templates, resource planning, budgets, and project billing. | §8.14 |
| Workflows-as-code | YAML HWF from Phase 4; TypeScript SDK (compiles to HWF) in Phase 7, once system packs have exercised HWF. | §6.7, ADR-016 |
| Chat / phone intake | Not needed early. Post-1.0 candidate. Workflows and webhooks can bridge in the meantime. | Plan "Beyond 1.0" |
| PSA importers | Eventually, not soon. Post-1.0 candidate, delivered as packs. | Plan "Beyond 1.0" |
| Domain | `usehuxley.io` | Header, §15 |
| Automation-only deployments | Every non-core feature is a module that can be turned off, instance-wide or per provider org. | §8.0, ADR-017 |
| Observability | OpenTelemetry throughout | §13.3, ADR-015 |
| Script sandboxes (2026-11-23) | Runners start each script in a rootless Podman container inside the runner container by default; Kubernetes Jobs (with a gVisor runtime class where available) are an option for installs on Kubernetes. Runners never get the Docker socket. | §6.9 |

### 18.1.1 Resolved later

| Question | Decision | Where reflected |
|---|---|---|
| Skills, certifications and timesheets (2026-11-24) | Added as the `workforce` module. Skills, certifications and availability come in Phase 5, with dispatch. Timesheets come in Phase 6, with agreements. Dashboards are in Phase 7, and AI dispatch and skill gaps in Phase 8. | §8.15, plan Phases 5–8 |

### 18.2 Still open

None currently. New questions get added here as they come up.
