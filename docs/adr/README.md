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
| [022](0022-visual-workflow-editor-as-nested-blocks.md) | The visual workflow editor shows nested blocks, not a free-form graph | Superseded by 034 |
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
| [034](0034-workflow-canvas-with-derived-edges.md) | The visual workflow editor is a SvelteFlow canvas whose edges come from the document | Accepted |
| [035](0035-crm-records-belong-to-organizations.md) | CRM records belong to organizations; contacts can link to several | Accepted |
| [036](0036-tickets-types-statuses-and-queues.md) | Tickets, with per-type statuses, service queues and a priority matrix | Accepted |
| [037](0037-sla-clocks.md) | SLAs as per-ticket clocks over a snapshot calendar | Accepted |
| [038](0038-email-intake.md) | Email intake: mailboxes, stored messages, and email to ticket as a step | Accepted |
| [039](0039-agreement-coverage.md) | Agreements evaluate time on demand, one agreement per entry | Accepted |
| [040](0040-portal-realm.md) | The client portal is a separate realm: its own accounts, sessions and API | Accepted |
| [041](0041-semantic-data-layer.md) | A semantic data layer: a catalog of sources and a query spec compiled to SQL | Accepted |
| [042](0042-hwf-v1-frozen.md) | HWF v1 is frozen; later changes only add | Accepted |
| [043](0043-csat-surveys.md) | Satisfaction surveys are sent by a workflow step and answered by link or in the portal | Accepted |
| [044](0044-client-health.md) | Client health is a weighted average of the factors that have data | Accepted |
| [045](0045-dashboard-publishing.md) | Dashboards are published by approving a snapshot for an audience | Accepted |
| [046](0046-sentiment.md) | Sentiment is judged by a workflow and recorded by a step | Accepted |
| [047](0047-ai-triage.md) | AI triage is measured by running its own workflow on labeled samples | Accepted |
| [048](0048-dispatch-recommendations.md) | Dispatch recommendations are a transparent score, not a model's guess | Accepted |
| [049](0049-skill-gaps.md) | Skill gaps are computed from requirements, not guessed | Accepted |
| [050](0050-ticket-insights.md) | Ticket insights: an AI summary by workflow, similar tickets by embedding | Accepted |
| [051](0051-auto-documentation.md) | Auto-documentation drafts; people decide | Accepted |
| [052](0052-report-export.md) | Reports export without new dependencies, and schedule as workflows | Accepted |
| [053](0053-assistant-v2.md) | Assistant v2: tool groups by module, and context from where it's opened | Accepted |
| [054](0054-change-management.md) | Changes extend change tickets; approval is a workflow | Accepted |
| [055](0055-problem-management.md) | Problems extend problem tickets; incidents link to them | Accepted |
| [056](0056-projects.md) | Project tasks are tickets; budgets burn from time on them | Accepted |
| [057](0057-project-scope-and-views.md) | Scope changes are client-approved through a workflow; template workflows are filtered events | Accepted |
| [058](0058-project-capacity.md) | Capacity is computed from schedules, appointments and task estimates | Accepted |
| [059](0059-project-status-reports.md) | Status reports are drafted by a pack and shared by a person | Accepted |
| [060](0060-code-editor.md) | One CodeMirror 6 editor, loaded on demand | Accepted |
| [061](0061-calendar-sync.md) | Calendar sync reads free/busy, through connections and a pack | Accepted |
| [062](0062-calendar-write-back.md) | Appointments are written to calendars by an event-driven workflow | Accepted |
| [063](0063-sales-pipeline.md) | Leads live at the provider, opportunities at the client; stages are instance-wide | Accepted |
| [064](0064-quotes.md) | A quote is one editable document; each send keeps a revision | Accepted |
| [065](0065-quote-signing.md) | Clients answer quotes in the portal with a typed name and a drawn signature | Accepted |
| [066](0066-quote-review.md) | Quotes are reviewed internally by rules, a workflow and a third approval outcome | Accepted |
| [067](0067-inventory.md) | Inventory is a ledger of movements over stock per item, location and lot | Accepted |
| [068](0068-purchasing.md) | Purchase orders are approved by a threshold and received through inventory movements | Accepted |
| [069](0069-sales-orders.md) | Accepted quotes become sales orders, fulfilled through inventory and purchasing and handed off by the API | Accepted |
| [070](0070-sales-activities.md) | Sales activities live with their records, and sales email is logged from a mailbox by a workflow | Accepted |
| [071](0071-forecasting.md) | The forecast is first-year value by forecast category against monthly or quarterly quotas, and sales reporting uses the analytics sources | Accepted |
| [072](0072-presales-scoping.md) | Presales scoping drafts a templated Scope of Work with a workflow, keeps people's edits, and travels with the quote | Accepted |
| [073](0073-presales-estimates.md) | Estimates are three-point hours by priced role plus a sized bill of materials, drafted by the scoping pack and assembled into the quote | Accepted |
| [074](0074-scope-to-project.md) | A won deal's project is laid out from its scoped work breakdown, and the scope tracks estimate against actual | Accepted |
| [075](0075-invoicing.md) | Invoices are generated per client and currency from marked sources, drafts until approved, corrected by credit memos | Accepted |
| [076](0076-invoice-review.md) | Invoices are reviewed on the quote review model, and an approved review approves the invoice | Accepted |
| [077](0077-accounting-sync.md) | Accounting sync: Huxley prepares and tracks, a pack's system workflow creates | Accepted |
| [078](0078-payment-status.md) | Payment status comes back through the sync pack; Huxley keeps client balances and credit holds | Accepted |
| [079](0079-expenses.md) | Expenses are entries placed like time, with receipts kept beside them | Accepted |
| [080](0080-receipt-reading.md) | AI reads receipts into a suggestion the person confirms | Accepted |
| [081](0081-expense-reports.md) | Expense reports go to the person's team leads through an approval workflow | Accepted |
| [082](0082-expense-billing.md) | Approved billable expenses are billing sources; projects keep an expense budget | Accepted |
| [083](0083-reimbursement.md) | Expenses are paid back as a bill to the person through the accounting packs | Accepted |
| [084](0084-card-transactions.md) | Card charges are imported, deduplicated by bank ID, and matched to the cardholder's expenses | Accepted |
| [085](0085-retention.md) | A daily sweep enforces retention; the audit log is exported, not pruned | Accepted |
| [086](0086-audit-sealing.md) | Audit records are sealed into the hash chain after commit | Accepted |
| [087](0087-audit-streaming.md) | The audit log is streamed to SIEMs over HTTPS, in chain order | Accepted |
| [088](0088-scim-provisioning.md) | SCIM provisioning hangs off sign-in providers; groups are teams | Accepted |
| [089](0089-kubernetes-worker-pools-pgbouncer.md) | A Helm chart with worker pools by job class, KEDA, and PgBouncer | Accepted |
| [090](0090-client-address-and-shared-rate-limits.md) | Configurable client address behind CDNs; rate limits shared in PostgreSQL | Accepted |
| [091](0091-metrics-dashboards-alerts.md) | Application metrics over OpenTelemetry, with Grafana dashboards and tested alert rules | Accepted |
