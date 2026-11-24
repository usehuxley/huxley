# Architecture Decision Records

Decisions that shape Huxley. Add a new ADR by copying `0000-template.md` with the next number. To change a decision, write a new ADR that supersedes the old one rather than editing history.

| # | Decision | Status |
|---|---|---|
| [001](0001-rust-monolith-binary-with-role-subcommands.md) | Rust monolith binary with role subcommands | Accepted |
| [002](0002-postgresql-backed-job-queue-valkey-optional.md) | PostgreSQL-backed job queue, Valkey optional | Accepted |
| [003](0003-journal-based-durable-execution.md) | Journal-based durable execution | Accepted |
| [004](0004-hwf-as-the-single-canonical-workflow-format.md) | HWF as the single canonical workflow format | Accepted |
| [005](0005-minijinja-for-expressions.md) | minijinja for expressions | Accepted |
| [006](0006-cedar-for-authorization-evaluation.md) | Cedar for authorization evaluation | Accepted |
| [007](0007-scripts-run-only-in-isolated-runners.md) | Scripts run only in isolated runners | Accepted |
| [008](0008-datasets-as-generated-typed-tables.md) | Datasets as generated typed tables | Accepted |
| [009](0009-separate-sveltekit-apps-for-staff-and-portal.md) | Separate SvelteKit apps for staff and portal | Accepted |
| [010](0010-single-owner-instances-with-org-tree-tenancy.md) | Single-owner instances with org-tree tenancy | Accepted |
| [011](0011-system-automations-ship-as-packs-of-workflows.md) | System automations ship as packs of workflows | Accepted |
| [012](0012-mit-compatible-dependencies-only.md) | MIT-compatible dependencies only | Accepted |
| [013](0013-on-prem-execution-through-integrations.md) | On-prem execution through integrations | Accepted |
| [014](0014-huxley-generates-invoices-accounting-systems-are-the-system.md) | Huxley generates invoices; accounting systems are the system of record | Accepted |
| [015](0015-opentelemetry-as-the-only-instrumentation-standard.md) | OpenTelemetry as the only instrumentation standard | Accepted |
| [016](0016-typescript-sdk-compiles-to-hwf-at-build-time.md) | TypeScript SDK compiles to HWF at build time | Accepted |
| [017](0017-toggleable-feature-modules.md) | Everything outside the core platform is a toggleable feature module | Accepted |
| [018](0018-envelope-encryption-with-config-kek-and-per-provider-deks.md) | Envelope encryption with a configured KEK and per-provider-org data keys | Accepted |
| [019](0019-sso-account-linking-and-role-sync.md) | SSO account linking, role sync, and MFA trust | Accepted |
| [020](0020-structured-control-flow-in-hwf.md) | Structured control flow in HWF | Accepted |
| [021](0021-workflow-engine-execution-model.md) | Workflow engine execution model | Accepted |
| [022](0022-visual-workflow-editor-as-nested-blocks.md) | The visual workflow editor shows nested blocks, not a free-form graph | Accepted |
| [023](0023-integration-definitions-from-openapi.md) | Integration definitions come from OpenAPI through a tolerant importer | Accepted |
| [024](0024-callbacks-hand-their-url-over-in-the-step.md) | A callback step hands its URL over itself | Accepted |
| [025](0025-runner-identity.md) | Runners prove who they are with client certificates from Huxley or from you | Accepted |
| [026](0026-kubernetes-jobs-sandbox.md) | On Kubernetes, scripts run as confined Jobs, with egress through a proxy enforced by NetworkPolicy | Accepted |
| [027](0027-per-organization-connections.md) | One partner connection serves every client through organization variables | Accepted |
| [028](0028-packs.md) | Packs are signed zips whose manifest lists every file's hash | Accepted |
| [029](0029-datasets-as-managed-tables.md) | Datasets are managed tables whose DDL the database generates | Accepted |
| [030](0030-mcp-tools-call-the-api.md) | MCP tool calls go through the API, as the user | Accepted |
| [031](0031-ai-providers-bring-your-own-key.md) | AI providers are per organization, bring your own key | Accepted |
| [032](0032-oauth-for-mcp-clients.md) | Huxley is the OAuth authorization server for its own MCP endpoint | Accepted |
| [033](0033-lookups-are-recorded-per-run.md) | A run records each `lookup()` answer the first time it asks | Accepted |
