# Huxley: Implementation Plan

| | |
|---|---|
| **Status** | Draft v0.3 |
| **Date** | 2026-11-21 |
| **Companion** | `DESIGN_DOCUMENT.md` (architecture and design details; section refs like §6.4 point there) |

---

## 1. Strategy

This is a long-term project. The plan follows four rules:

1. **Build the engine first and make it useful on its own.** The automation engine ships as a standalone release (**Huxley Automate**) before any PSA module. That gives early users and feedback, and proves the foundation that every later module stands on.
2. **Each module is built on the engine from day 1.** Mailbox polling, SLA escalation, AI triage, etc. are system workflows (packs), never hidden background code. If a module needs something the engine can't do, extend the engine.
3. **Vertical slices.** Each phase delivers working backend, API, UI, tests, and docs for its scope, plus deployability. A half-built backend with no UI doesn't count as done.
4. **Security and reliability are not a later phase.** Authz, audit, encryption, tests, and observability are part of every phase's definition of done.

### Release milestones

| Milestone | Phases | What users get |
|---|---|---|
| **M1: Foundations** (internal) | 0–1 | Deployable shell: login, orgs, RBAC, audit |
| **M2: Huxley Automate Alpha** | 2–3 | Workflow engine, canvas, integrations via OpenAPI, forms, webhooks |
| **M3: Huxley Automate Beta** | 4 | Datasets, MCP server, AI assistant, workflows-as-code, packs |
| **M4: Service Desk Alpha** | 5–6 | Ticketing, queues, SLA, email ingestion, client 360 + Actions, CMDB, KB, contracts |
| **M5: Service Desk Beta** | 7–8 | Client portal, TypeScript SDK, AI ticket features, CSAT/NPS, client health, reporting |
| **M6: Operations Suite** | 9–10 | Change mgmt, problem mgmt, project mgmt, sales/CPQ, inventory, procurement |
| **M7: 1.0** | 11–12 | Invoice generation + accounting sync (QBO/Xero), scale/HA hardening, stable APIs |

---

## 2. Definition of Done (applies to every phase)

- [ ] API endpoints documented in OpenAPI and covered by integration tests (real Postgres via testcontainers)
- [ ] Every endpoint and `core.*` action enforces authorization; list endpoints use authz query filters
- [ ] Config and entity changes write audit events
- [ ] Domain events emitted for all state changes
- [ ] Feature-module code registers through the module registry, and the app still works with that module turned off (from Phase 5)
- [ ] UI built with the shared design system: keyboard accessible, light/dark, responsive
- [ ] Playwright E2E tests for the main user flows
- [ ] Metrics, traces, and structured logs for new background work
- [ ] Migrations follow expand/contract; no breaking change to published API without a version bump
- [ ] User docs plus an ADR for any significant decision
- [ ] `docker compose up` still produces a working install

---

## 3. Phases

### Phase 0: Project foundations

**Goal:** a repo, toolchain, CI, and design system that make the rest fast and consistent.

- Create the monorepo layout (design §15): Rust workspace, pnpm workspace, `apps/web`, `apps/portal` (stub), `packages/ui`.
- Tooling: `rustfmt`, `clippy` (deny warnings), `cargo-deny` (MIT-compatible license allowlist + advisories), `cargo-nextest`, Prettier/ESLint/svelte-check, pre-commit hooks, Renovate.
- CI (GitHub Actions): build, lint, and test for Rust and TS; Playwright smoke; Docker image builds (amd64 first; arm64 once native arm runners are available); Grype vulnerability scan (Anchore), chosen over Trivy after Trivy's 2026 supply-chain compromise; SBOM via Syft; cosign signing on tags. All third-party actions pinned to commit SHAs.
- Dev environment: `docker-compose.dev.yml` (Postgres + Valkey + Mailpit + OTel Collector + Grafana LGTM for local traces, metrics, and logs; object storage joins when attachments arrive in Phase 1, using the local-filesystem backend until then), a `just`/`make` task runner, seed data script.
- `huxley-server` skeleton: config loading (env + `huxley.toml` + `*_FILE`), OpenTelemetry setup (design §13.3: traces, metrics, and logs via OTLP, `tracing` bridge, W3C context propagation, redaction layer for telemetry attributes), health endpoints, graceful shutdown, `serve --roles`, `migrate`.
- `huxley-db`: pool, transaction helpers, migration runner, RLS session context.
- API conventions: error format (RFC 9457 problem+json), pagination (cursor), filtering/sorting grammar, idempotency-key support, `utoipa` OpenAPI, generated `packages/api-client`.
- Design system v0: tokens, theme (light/dark), layout shell (sidebar, top bar, command palette stub), core components (button, input, select, table/data grid, dialog, toast, tabs, form controls).
- Repo docs: `README`, `LICENSE` (MIT), `CONTRIBUTING`, `SECURITY.md`, `CODE_OF_CONDUCT`, ADR template, ADRs 001–016 from the design doc.

**Exit criteria:** CI green on an empty-but-real app; `docker compose up` serves a login page placeholder and a healthy API.

---

### Phase 1: Platform core (identity, orgs, RBAC, security primitives)

**Goal:** the secure multi-org base everything else depends on.

- **Org tree** (design §4.4, §5): organizations with `ltree` paths and `kind` (provider/client), support for an owning company with multiple provider (MSP) orgs, CRUD, move, org switcher UI, org settings with inheritance and lock, per-provider-org branding and numbering prefixes, RLS policies on org scope.
- **Authentication** (§11.2): local users (Argon2id), email verification and reset (via SMTP config), TOTP + WebAuthn MFA, sessions, a device list, OIDC with an Entra preset (claim/group → role mapping, JIT provisioning), PATs and service accounts.
- **Authorization** (§11.3): permission registry (modules declare their permissions), roles (defaults + custom), scoped assignments with inheritance, Cedar policy compilation, `Authorizer` point checks + SQL filter generation, field-level permission stripping, an "effective permissions" explainer UI.
- **Teams** (groups of users) usable in assignments.
- **Crypto** (§12): envelope encryption, KMS providers (env/file + Azure Key Vault), key rotation command.
- **Audit log**: append-only, hash-chained, viewer UI with filters, export.
- **Custom fields, tags, attachments framework** (object storage via `object_store`).
- **Module registry and toggles** (design §8.0): module manifests, enable/disable with dependency checks (instance-wide, optionally per provider org), API gating (`module-disabled`), navigation gating, a modules admin page, and bootstrap presets (*Automation only*, *Service desk*, *Full PSA*). No toggleable modules exist yet; this lays the foundation so every module from Phase 5 on registers behind a toggle from day one.
- **Events outbox** + dispatcher skeleton.
- **Notifications** v0: in-app notifications + email channel, a real-time channel (WS/SSE with Valkey or PG LISTEN fan-out).
- Admin UI: users, teams, roles, org tree, auth providers, system settings.

**Exit criteria:** an admin can set up an MSP root and client orgs, configure Entra SSO, create roles scoped to orgs, and every action shows in the audit log. Security review of the auth flows completed. Authz tests cover inheritance and denial cases.

---

### Phase 2: Automation engine MVP

**Goal:** durable workflows that can be built on a canvas and run reliably.

- **HWF** (§6.2): Rust types, JSON Schema publication, validation (graph checks, reference resolution, type hints).
- **Expressions** (§6.3): minijinja environment, custom filters, typed evaluation, secret taint and redaction, sandbox limits.
- **Engine** (§6.4): runs/step_runs (partitioned), jobs queue with SKIP LOCKED, heartbeats, reaper, retries/backoff, timeouts, cancellation, payload spill to object storage, per-org concurrency limits, resume-from-failed-step.
- **Scheduler role**: leader election, cron/interval triggers, timers for `wait`.
- **Triggers**: manual, schedule, webhook (HMAC/token/basic auth, sync and async response modes), domain event.
- **Steps**: `http.request` (with SSRF egress policy), `transform`, `set_var`, `if`/`switch`, `foreach` (with concurrency), `parallel`/`join`, `try`/`catch`, `wait`, `subworkflow`, `return`/`fail`, `core.notify`.
- **Workflow lifecycle**: draft, publish (immutable versions), archive, version history and diff, run-as identity.
- **Canvas** (`packages/canvas`, SvelteFlow): node palette, connect/branch, auto-layout, node config panels generated from schemas, expression editor with autocomplete and preview, validation overlay, copy/paste, sticky notes.
- **Run experience**: test runs with pinned data, run-from-node, live run view (step status streaming), run history list with filters, step I/O inspector, re-run/resume, dead-letter view.
- **Organization variables** (§6.6) with inheritance and lock, including secret variables.

**Exit criteria:** a user builds a workflow on the canvas that receives a webhook, calls two HTTP APIs with retries, branches, loops, and notifies. Killing workers mid-run results in correct resumption with no lost steps (chaos test in CI). Load test: a sustained target throughput (e.g., 100 steps/sec on a modest single node) documented.

---

### Phase 3: Integrations, scripts, forms (extensibility)

**Goal:** connect to any system and let people (and technicians) trigger automations through forms.

- **OpenAPI import** (§7.1): parse 3.0/3.1 + Swagger 2.0 conversion, generate actions and auth schemes, overlay editor (rename/hide, pagination, rate limits, pollable lists), re-import with diff that keeps overlays.
- **Connections** (§7.2): encrypted credential instances, OAuth2 auth-code (PKCE) + client credentials + API key + basic/bearer, token refresh with distributed lock, health checks, org inheritance/sharing, the partner-credential + child-tenant pattern.
- **`integration.action` step** with schema-driven config UI, pagination helpers, rate limiting, idempotency headers.
- **Integration poll trigger** with cursor/dedupe state.
- **Script library + runners** (§6.9): `huxley-runner` gRPC service, `runner-pwsh` and `runner-python` images, sandbox hardening, credential injection, log streaming, versioned scripts with parameter schemas, the `script` step.
- **Callbacks**: `wait_for_callback` with signed single-use URLs.
- **Forms** (§6.8): form builder, JSON-Schema renderer (`packages/form-renderer`), dynamic options from actions, conditional fields, validation, internal and public audiences, submissions linked to runs.
- **Approval / human task step** (in-app + email link).
- **Templates**: workflow and form templates, "use template" flow.
- **First built-in integration packs**: Microsoft Graph (M365/Entra), Partner Center/GDAP, generic Webhook/HTTP, SMTP/email, Teams/Slack messaging.

**Exit criteria:** import a third-party OpenAPI spec and run its actions within minutes without code. A technician-facing "Create M365 User" form runs end-to-end against a test tenant using a PowerShell or Graph step. Third-party security review of the runner sandbox and SSRF controls.

**→ Release M2: Huxley Automate Alpha**

---

### Phase 4: Datasets, MCP, AI, workflows-as-code, packs

**Goal:** finish the automation platform. Data, AI-assisted building, code-first workflows, shareable packs.

- **Datasets** (§9): definitions, typed managed tables + DDL migrations, sync system workflow (full/incremental/upsert), per-descendant-org syncs, `dataset.query`/`dataset.upsert` steps, `lookup()` in Jinja, dataset browser UI, admin read-only SQL console.
- **AI provider layer** (§10.1): BYOK provider abstraction (Anthropic, OpenAI, Azure OpenAI, OpenAI-compatible), per-org config and inheritance, budgets, call logging, `ai.prompt`/`ai.classify`/`ai.extract` steps. (Embeddings + pgvector moved to Phase 5, where their first consumer is.)
- **MCP server** (§10.2): Streamable HTTP + stdio, OAuth 2.1 + PAT auth, automation tool group, workflows exposed as `mcp_tool`, drafts-only + human publish.
- **AI assistant** (§10.3) v1 in the web app: workflow building, debugging failed runs, and Q&A over platform data within the user's permissions.
- **Workflows-as-code** (§6.7): `huxley-cli` (validate/test/push/pull/diff/run), fixtures-based tests, YAML code view in-app with canvas round-trip, per-org Git sync (GitHub/GitLab/Azure DevOps).
- **Packs** (§6.6): `.hxpack` format, signing, import/export/upgrade, install review UI (shows egress domains, scripts, permissions), and moving built-in integrations into packs.
- **Runner `js`** (Node.js; Deno was the first choice, but its team announced the end of its development in October 2026).
- **Docs site** v1 at docs.usehuxley.io: concepts, tutorials, integration and pack authoring guide, HWF reference. HWF JSON Schema published at `usehuxley.io/schemas/hwf/v1.json`.

**Exit criteria:** an external MCP client (e.g., Claude Code with the user's own key) can build, test, and submit a working workflow draft. A Git-synced repo can publish workflows through CI. A dataset of M365 users for every client org syncs on schedule and appears in a form dropdown.

**→ Release M3: Huxley Automate Beta**

---

### Phase 5: Service desk core

**Goal:** a usable ITIL-aligned service desk built on the engine.

- Every module in this phase (`crm`, `servicedesk`, `workforce`) registers through the module registry and ships with a CI job that runs the suite with the module off, so *Automation only* installs stay healthy.
- **CRM base** (§8.1): companies (client orgs), sites, contacts, contact ↔ company links, client 360 view shell.
- **Ticketing** (§8.2): ticket types (Incident, Service Request; Problem/Change stubbed), configurable status workflows, service queues + membership, priority matrix, notes (public/internal), time entries, attachments, parent/child/related, merge, templates, canned responses, bulk edit, saved queue views, My Work, collision/presence indicators.
- **SLA engine**: policies, business-hours calendars and holidays, pause statuses, clocks driven by the scheduler, warning and breach events, SLA display everywhere.
- **Mailbox ingestion** (§8.4): M365 (Graph delta + subscriptions) and IMAP (IDLE/poll) mailboxes, "Poll mailbox" + "Email to ticket" system workflows, threading, contact matching, loop/auto-reply protection, outbound replies via Graph/SMTP.
- **Workforce base** (§8.15, `workforce` module):
  - The skills catalog (levels) and people's skills (self-assessed or verified).
  - The certification catalog and people's certifications (expiry, credential, evidence). `certification.expiring` and `certification.expired` events, with a reminder workflow.
  - Skill and certification requirements on queues and ticket types, and a skills matrix view.
  - Working schedules and time off (approved through the approval step).
  - Work types for non-ticket time entries.
- **Assignment/dispatch** system workflows: round-robin, load-balanced, skill-based (skill requirements weighed, required certifications as hard constraints, people away or off-hours skipped). Dispatch calendar v1.
- **Client 360 Actions panel**: forms flagged `action` with automatic context injection (org/contact/asset/ticket). Ticket macros run workflows.
- **`core.*` actions** for tickets, contacts, companies. Domain events for all of them.
- **Command palette + global search** (Postgres FTS + trigram).
- **Embeddings + pgvector** (moved from Phase 4, decided 2026-11-25): pgvector stays optional (semantic features switch off without it); an embeddings call in `huxley-ai` (OpenAI, Azure OpenAI, OpenAI-compatible, Voyage) with its own provider setting, since Anthropic has no embeddings API; semantic ticket search alongside FTS as the first consumer. Knowledge-base search (Phase 6) and similar tickets (Phase 8) build on it.
- **Notifications** expanded: ticket assignment/update rules as system workflows, Teams/Slack channels.

**Exit criteria:** a small MSP can run day-to-day support in Huxley. Email in → ticket → triage to queue → SLA tracked → technician works it from My Work, runs a "Reset MFA" Action from the client view, replies by email → resolves. All automation is visible as system workflows.

---

### Phase 6: CMDB, knowledge, contracts, warranties

- **Assets/CIs** (§8.5): CI types with custom schemas, lifecycle, relationships, **dependency map** (SvelteFlow) with impact analysis, asset tab in client 360, link assets to tickets.
- **Dataset → asset mapping rules** (match keys, merge policy, field provenance).
- **First RMM integration pack(s)** (design §6.9, §7.3): device discovery, plus script and job execution actions so workflows can run scripts on customer networks through the RMM. Example system workflow: "Run script on device and attach output to ticket".
- **Knowledge base**: articles, versions, review/approval workflow, visibility levels, ticket linking, FTS + semantic search.
- **Client documentation** area + optional credential vault (audited reveals).
- **Agreements/contracts** (§8.7): types, coverage rules, rate cards, SLA binding, time-entry coverage evaluation, renewal events.
- **Timesheets** (§8.15):
  - Periods, weekly by default.
  - Submit → review by lead or approver (the approval step) → approve or reject with comments. Approved timesheets lock their entries; reopening is audited.
  - Reminder workflows for missing or late timesheets and under-logged days.
  - An option to bill only approved time, and a payroll export.
- **Warranties**: per asset, vendor lookup workflow (Dell/Lenovo/HP packs), expiry events.
- **Product catalog** (basic; needed by agreements and later by inventory/CPQ).
- **Workflow folders** (added 2026-11-25, at the user's request):
  - **Folders:** per organization, nestable, to organize the Workflows list. A workflow is in one folder, or none (unfiled, at the top level). Keeping folders (create, rename, move, delete) needs `workflow:manage` there. Deleting a folder moves its contents to its parent; it never deletes workflows.
  - **The System folder:** every organization has one, made by Huxley. It can't be deleted, renamed or moved.
    - It holds the workflows installed from Huxley's built-in packs (their installs are recorded in `pack_installs`): new installs go there automatically, and existing ones are moved there by a migration.
    - It holds only those: your own workflows can't be moved into it, and system workflows stay in it. System workflows can still be published, paused and edited as today.
  - **Other packs:** a third-party pack's workflows go into a folder named after the pack, created if needed, which can then be renamed, moved or deleted like any folder.
  - **The Workflows list** shows the folder tree beside the list, with counts. Search and status filters cover every folder. Moving workflows works one at a time or in bulk.
  - **API:** `/api/v1/workflow-folders`, a workflow's `folderId`, and a `folderId` filter on the workflow list. Workflows-as-code exports keep the folder path.

**Exit criteria:** assets sync from an RMM, map into CIs with relationships, and render in the dependency map. Time entries on tickets correctly evaluate as covered or billable under an agreement. A technician's weekly timesheet is submitted, rejected with a comment, corrected, approved and locked, and only its approved time reaches a billing preview.

**→ Release M4: Service Desk Alpha**

---

### Phase 7: Client portal, reporting foundation, TypeScript SDK

- **Portal app** (§8.13): white-labeling (logo, colors, custom domain), portal auth realm (magic link/password + MFA, client OIDC by domain), portal RBAC roles, company-admin contact management.
- Portal features: tickets (view/create/comment), service catalog (portal-audience forms), KB, assets, approvals.
- **Semantic data layer** (§8.12): data source catalog for core entities + datasets, structured query spec → SQL compiler with authz filters, cost limits, caching, read-replica routing.
- **Widget renderer** (`packages/widgets`) shared by web and portal. Prebuilt dashboards (service desk overview, SLA performance, technician utilization, client overview, and **time and utilization**: billable %, hours by client/agreement/work type, overtime and under-logging, estimate vs actual).
- **HWF v1 stabilization + TypeScript SDK** (design §6.7, ADR-016):
  - **Why here:** by this point the engine has been used for real by the system packs behind service desk, email, SLA, CMDB, and contracts (Phases 5–6), so HWF has settled. Building the SDK earlier would mean chasing format changes. Building it later would delay the code-first audience and community pack authors.
  - Freeze HWF `usehuxley.io/v1`, with a compatibility policy for later additions.
  - `@usehuxley/sdk` (`packages/sdk`): typed builders for steps, control flow, triggers, inputs, and forms. Typed Jinja reference helpers.
  - `huxley-cli codegen`: generates TS types for an instance's installed integrations and actions.
  - `huxley-cli build`: deterministic TS → HWF compilation, integrated with `test`, `push`, and Git sync.
  - Rewrite a few built-in packs in TS as reference examples. SDK docs and a starter template repo.

**Exit criteria:** a client contact logs in via their own Entra tenant, sees only their company's permitted tickets and assets, submits a service-catalog form that runs a workflow, and views a published dashboard. A workflow written with `@usehuxley/sdk` against generated integration types compiles to HWF, passes `huxley-cli test`, publishes via Git sync, and opens correctly on the canvas.

---

### Phase 8: AI ticket intelligence, customer experience, report builder

- **AI features as system workflows** (§8.8): sentiment analysis, triage (suggest/auto modes, including the skills a ticket needs), dispatch recommendations (skills, certifications, availability, load, history and client familiarity, with reasons), skill-gap insights (unqualified work, expiring certifications), ticket insights (summary, entities, similar tickets, KB and Action suggestions), auto-documentation (resolution notes, KB drafts into review, draft time entries for timesheets).
- Assistant v2: context-aware on tickets and client views, service desk + clients/assets MCP tool groups.
- **CSAT** (§8.9): surveys on resolve, email one-click + portal, sampling rules.
- **NPS** campaigns.
- **Client health score**: configurable model, score + trend + reasons, `client.health_declining` event + default account-manager workflow.
- **Widget/dashboard/report builder** UI, scheduled reports, CSV/XLSX/PDF export, **publishing workflow** (submit → approve → publish to roles/orgs/portal).

**Exit criteria:** AI triage accuracy measured on a labeled sample set (target set during the phase). A declining-sentiment client is flagged with explanations. A user builds a dashboard, submits it, and after approval it appears for a team and in the client portal.

**→ Release M5: Service Desk Beta**

---

### Phase 9: Change, problem and project management

- **Change management** (§8.6): change types (standard/normal/emergency), risk questionnaire, affected CIs from the dependency map, approval policies/CAB using approval steps (including client approvers via portal/email), change calendar, freeze windows, conflict detection, PIR.
- **Problem management**: problem records, linking incidents, known errors, root cause, workaround → KB.
- **Project management** (§8.14):
  - Projects linked to company/agreement/opportunity.
  - Phases, tasks, milestones, and dependencies. Project tasks are tickets (*Project Task* type) that appear in My Work.
  - Project templates with attached workflows.
  - List, board, timeline/Gantt, and calendar views.
  - Resource capacity planning shared with the dispatch calendar, matching task skill requirements to people (§8.15).
  - Hour/cost budgets vs actuals from time entries, plus budget threshold events.
  - Billing method settings (fixed fee, milestone, T&M, not-to-exceed) for use in Phase 11.
  - Scope change requests with client approval.
  - Portal project view.
  - Scheduled, AI-drafted status reports.

**Exit criteria:**
- A high-risk normal change routes to CAB, shows impacted CIs, respects a freeze window, and is approved by a client approver in the portal.
- A client onboarding project created from a template:
  - assigns tasks across technicians, who see them in My Work
  - shows budget burn from time entries
  - fires a budget threshold event
  - displays milestone status in the client portal

---

### Phase 10: Sales, CPQ, inventory and procurement

- **Leads → opportunities** (§8.10), pipeline stages, kanban + list, win/loss.
- **Activities & communications**: tasks, calls, meetings, email logging (mailbox sync + BCC address).
- **Forecasting**: weighted pipeline, categories, quotas.
- **CPQ**: quote builder, bundles, distributor pricing via integration packs (Pax8/Ingram/TD SYNNEX), margin rules, recurring vs one-time, options, branded PDF templates, versioning, client acceptance page with e-signature + audit trail.
- **Inventory** (§8.7): warehouses/locations, stock levels, serial/lot tracking, movements, allocation to tickets/projects, reorder alerts.
- **Purchase orders**: vendors, POs (consolidated by vendor from sales orders, or manual), approval, receiving into inventory, partial receipts.
- **Sales orders**: from accepted quotes → generate POs → receive → allocate/fulfill → hand-off to agreements (recurring lines), projects (create from template), and invoice generation.
- Sales dashboards and reports.

**Exit criteria:** opportunity → quote → client e-accepts in the portal → sales order → POs to vendors → receive into stock → allocate and fulfill. Recurring lines create or update an agreement, and an onboarding project is created from a template.

**→ Release M6: Operations Suite**

---

### Phase 11: Invoice generation and accounting sync

- **Invoice generation** (§8.11). Sources:
  - Time entries, evaluated for agreement coverage.
  - Expenses.
  - Project billing: fixed fee, milestone, T&M, not-to-exceed.
  - Sales order lines.
  - Agreement recurring charges, with proration and dataset-driven usage quantities.
- **Billing runs**: generate drafts → review (edit lines, hold, PDF preview) → approve → sync. Billed sources are marked to prevent double billing. Unbilled work-in-progress view.
- **Accounting sync packs**: QuickBooks Online and Xero first, built as integrations, with the sync as a system workflow so other platforms can be added. Includes:
  - A mapping UI (companies ↔ customers, products/work types ↔ items, tax codes, classes/tracking categories).
  - Per-invoice sync status, an error queue, and retry.
  - Credit memos for corrections.
- **Status back-sync**: paid, balance, and overdue status pulled from the accounting system onto companies. Feeds the client portal, client health, and optional credit-hold events.
- Portal: invoice list with synced status and links to the accounting system's view/pay page.
- Finance dashboards: billed vs unbilled, agreement and project profitability, technician utilization vs billable.

**Exit criteria:** a monthly billing run for a set of clients produces correct invoice drafts, verified against fixture scenarios covering agreements, overages, project milestones, and product sales. The invoices sync to QuickBooks Online and Xero sandboxes with correct customer, item, and tax mappings, and payment status flows back into Huxley.

---

### Phase 12: Scale, hardening, migration, 1.0

- **Scale/HA**: Helm chart with KEDA autoscaling, worker pools by job class, PgBouncer, read replicas, partition maintenance automation, retention jobs, published performance benchmarks for small/medium/large reference sizes.
- **Deployment templates**: DigitalOcean (App Platform/DOKS + managed services, Marketplace image), Azure (Container Apps + Flexible Server + Key Vault, Bicep/Terraform).
- **Security**: external penetration test, ASVS L2 gap review, threat model refresh, SIEM audit streaming, SCIM provisioning.
- **Observability**: production Grafana dashboards and alert rules in `deploy/observability/`, tail-sampling collector config, Azure Monitor wiring in Azure templates.
- **Community**: public pack registry, pack authoring docs, contribution guidelines for integrations.
- **API stability**: freeze `/api/v1`, deprecation policy, semantic versioning, upgrade guides.
- Accessibility audit (WCAG 2.2 AA), i18n framework with first community translations.

**→ Release M7: Huxley 1.0**

---

### Beyond 1.0 (candidates, not scheduled)

| Candidate | Notes |
|---|---|
| **PSA/documentation importers** | ConnectWise PSA, HaloPSA, Autotask, IT Glue/Hudu, delivered as packs. |
| **Chat and phone intake** | Teams chat, live chat widget, telephony (3CX, RingCentral, etc.) as ticket intake channels. Workflows and webhooks can bridge these before then. |
| **Huxley agent** | First-party on-prem agent (design §6.9). Only if integration-based execution proves insufficient. |
| **External MCP client step** | `mcp.call` step to call tools on external MCP servers from workflows. |

## 4. Cross-cutting tracks (run continuously)

| Track | Ongoing work |
|---|---|
| **Security** | Threat model updates per phase, dependency audits, secret-handling reviews, fuzzing of parsers (OpenAPI, email, HWF, Jinja). |
| **Reliability** | Chaos tests for the engine (worker kill, DB failover, Valkey loss), soak tests, backup/restore drills. |
| **Observability** | Every new component and background job ships with OpenTelemetry spans, metrics, and log correlation. Dashboards and alerts are updated alongside features. |
| **Performance** | Benchmarks in CI for the engine and key queries; regression alerts. |
| **UX/design** | Design system growth, usability reviews per module, keyboard-first parity. |
| **Docs** | User docs, admin/deployment docs, API reference, ADRs, examples. |
| **Packs** | Grow the built-in integration and workflow library alongside each module. |

---

## 5. Testing Strategy

| Level | Approach |
|---|---|
| Unit | Rust `#[test]` + `nextest`; Vitest for TS/Svelte components |
| Integration | Real PostgreSQL and Valkey via testcontainers; API tests against the running router |
| Authz | Table-driven permission tests per endpoint (allowed/denied/filtered), plus property tests on inheritance |
| Engine | Deterministic tests with fake clock + fault injection (kill/restart mid-step), idempotency checks |
| Contract | Generated API client compile checks; OpenAPI diff in CI to catch breaking changes |
| E2E | Playwright across web + portal for the main flows of each phase |
| Integrations | Recorded HTTP fixtures (VCR-style) + optional live sandbox tests (M365 dev tenant) on a schedule |
| Security | `cargo-audit`, `cargo-deny`, ZAP baseline scan on the compose stack, fuzzing (`cargo-fuzz`) |
| Performance | k6/Goose load tests for API and engine throughput |

---

## 6. Immediate Next Steps

1. All design questions are resolved (design §18.1). The `usehuxley` GitHub org exists. Still to reserve: the `@usehuxley` npm scope, and DNS for `docs.` and `packs.usehuxley.io`.
2. Create the repositories in `github.com/usehuxley`: `huxley` (monorepo) and `huxley-packs`. Add the MIT license, README, and this design doc + plan under `docs/`.
3. Record ADR-001 → ADR-016 from the design doc.
4. Begin **Phase 0**: toolchain, CI, dev compose stack, server skeleton, design system v0.
5. Prototype the two riskiest pieces early, in parallel with Phase 0/1:
   - **Engine durability spike**: Postgres queue + journal executor + kill-test.
   - **Canvas spike**: SvelteFlow node editor with schema-driven config panel and Jinja expression editor round-tripping to HWF YAML.
