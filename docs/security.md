# Security

How Huxley protects itself and its data. Reporting a vulnerability: see [SECURITY.md](https://github.com/usehuxley/huxley/blob/main/SECURITY.md).

This page holds the threat model (STRIDE, by component), the gap review against OWASP ASVS Level 2 (the design's target), and how Huxley handles hostile input. It describes the code as it is; update it with every change that adds a component, a trust boundary or a control.

## Threat model

### What's at stake

- **Clients' credentials.** Connections hold API keys and OAuth tokens to clients' systems: Microsoft 365, RMMs, firewalls. Someone who could use them could act in every client's environment. This is the most valuable thing Huxley holds.
- **Clients' data.** Tickets, contacts, assets, documents, vault entries, invoices and expenses, separated by organization.
- **Control of automation.** Publishing a workflow, or running a script on a runner, acts on clients' systems.
- **Integrity of the record.** The audit log, which is evidence after an incident, and invoices.

### Who's involved

| Actor | Trust |
|---|---|
| Anyone on the internet | None. They reach sign-in pages, public form links, webhooks, the OAuth endpoints for MCP, and SCIM (with a token). |
| Portal contacts (clients' staff) | Their own organization's portal data, through a separate sign-in realm. |
| Staff (technicians and others) | What their roles grant, at the organizations they're assigned (scoped, inherited down the tree). |
| Admins and owners | Instance-wide settings. Owners hold everything. |
| Workflow authors | Write expressions and HTTP steps that run with an organization's connections. |
| Identity providers | Sign people in (OIDC) and provision them (SCIM). |
| Third-party APIs | Answer integration calls. Their responses are untrusted. |
| Script runners | Run scripts in sandboxes, on customers' networks. |
| Operators | Run the servers. They can read configuration, the database and the key-encryption key (KEK), unless it's in a key service. |

### Trust boundaries

1. The browser and the web or portal apps.
2. The internet and the API: webhooks, SCIM, MCP, OAuth.
3. The API and PostgreSQL. Row-level security is enforced in the database, not only in code.
4. The workflow engine and the outside world: HTTP steps and integrations, through the egress policy.
5. Huxley and script runners: mutual TLS, and runners pull their work.
6. The data and the key that protects it: envelope encryption, with the KEK optionally in a key service.

### By component

**Threat letters (STRIDE):** **S**poofing, **T**ampering, **R**epudiation, **I**nformation disclosure, **D**enial of service, **E**levation of privilege.

| Component | Threats | Controls | Residual risk |
|---|---|---|---|
| **Staff sign-in** (passwords, MFA, passkeys, SSO) | S: guessed or stuffed passwords, stolen sessions. E: reaching admin. | Argon2id (OWASP parameters). New passwords need at least 12 characters and 4 different ones, and are refused if they're among the ~30,000 breached passwords of that length in a bundled list (checked offline, ignoring case). Lockout after 5 failures, doubling from 60 s to 1 h. Per-address rate limit, shared by every replica (ADR-090). MFA (TOTP, passkeys), which a role or organization can require. Sessions: random 256-bit tokens, stored hashed; cookie `HttpOnly; SameSite=Lax; Secure`; 8 h idle, 7 days absolute. Disabling a user revokes their sessions at once. SSO with PKCE, state and nonce. | The bundled list covers the most common breached passwords, not every one ever leaked. |
| **Portal sign-in** | S, E: a client contact reaching staff data or another client. | A separate realm: its own sessions, cookie and API (`/api/portal/v1`). Data scoped to the contact's company. | — |
| **API tokens and OAuth (MCP)** | S: token theft. E: a token doing more than its owner. | Tokens are random and stored hashed. Scopes narrow a token below its owner's permissions, never above. OAuth requires PKCE (S256). MCP tool calls go through the API as the user, are audited `via = mcp`, and only touch drafts. | Long-lived tokens until revoked. |
| **SCIM** | S: a forged identity provider. E: provisioning an owner. | A per-provider bearer token, stored hashed and shown once. SCIM can't grant roles: roles stay with teams, set in Huxley. Deprovisioning disables and revokes sessions. | Whoever holds the token controls who exists. Rotate it if exposed. |
| **Authorization** | E: reading or changing another organization's data. I: fields a role shouldn't see. | Permission checks at every endpoint. **Row-level security in PostgreSQL** on organization-scoped tables, keyed to the request's scope, as a non-owner role. Field-level permissions. Owner can't be granted to teams or service accounts, and the last owner can't be removed. Behind a transaction pooler, startup refuses to run if connections don't use the restricted role (ADR-089). | A missing check in code is still stopped by row-level security for scoped data; instance-wide settings rely on the code check. |
| **Web apps** | T: CSRF, XSS, clickjacking. | State changes need an anti-forgery header (`X-Requested-With`), or a bearer token, which a browser can't send cross-site. CSP with nonces. `X-Frame-Options: DENY`, `nosniff`, a strict referrer policy. Svelte escapes output. Rich text is sanitized server-side. | — |
| **Workflow engine** | I, E: an author using HTTP steps against internal services (SSRF), or cloud metadata. D: runaway runs. | Egress policy: only public addresses by default. DNS answers are checked, and connections go to the checked address (no rebinding). Link-local and metadata addresses are always blocked. Explicit allow and deny lists. Expressions are bounded by fuel, recursion and output size. Runs have timeouts and per-workflow concurrency. Secrets are redacted from run records. | An author can still reach any public host with the organization's connections. Publishing is a separate permission. |
| **Script runners** | E: a script escaping to the runner's host or network. S: a fake runner taking work. | Rootless Podman (or Kubernetes) sandboxes: no network of their own (only the egress proxy, with the same policy), read-only root, all capabilities dropped, `no-new-privileges`, memory and process limits. Runners enroll with single-use tokens and authenticate with short-lived client certificates (mutual TLS) from Huxley's own CA, or yours. On Kubernetes, runners refuse to run anything unless the network policy is in force. | Container escapes in the kernel or Podman are out of Huxley's hands; keep runners patched. |
| **Inbound webhooks and callbacks** | S: forged events. T: replays. | Per-trigger HMAC (SHA-256/512), bearer or basic auth, with constant-time comparison. With a signed timestamp, requests older than 5 minutes are refused, and each signature is recorded for that window so a repeat starts no run. Callback URLs carry single-use random tokens. | Without a signed timestamp (the sender doesn't support one), a captured request can be replayed. Bearer and basic auth can't detect replays. |
| **Inbound email** | S: spoofed senders opening tickets as a contact. D: mail bombs. | Senders are matched to contacts, and unknown senders go to a configurable organization. Size limits. A hostile-input-tested parser. | Email sender addresses can be forged. Rely on your mail provider's SPF, DKIM and DMARC checks. |
| **Integrations and connections** | I: credential theft. T: a malicious API response steering a workflow. | Credentials are encrypted (see Data at rest), never returned by the API, and never put in packs or exports. Responses are treated as data: expressions can't run code. | — |
| **Packs and registries** | T: a tampered or malicious pack. E: a pack doing what its installer couldn't. | Every file is hash-listed, and an Ed25519 signature covers the manifest. Each pack is shown as trusted, signed by an unknown key, or unsigned. Reading is bounded (zip bombs). Every install is reviewed, showing hosts reached and scripts added. Installing needs the permissions for everything the pack adds. Workflows arrive as drafts. Registry downloads must match the index's SHA-256, and indexes can be signed (ADR-093). | An admin can still approve a malicious pack. The review shows what it does. |
| **AI features** | T: prompt injection from ticket text, email or web content steering a summary, triage or the assistant. I: data sent to an AI provider. | The assistant's tool calls run as the user, and anything that changes something waits for approval. AI providers are opt-in per organization, with your own keys, and usage is recorded and budgeted. | **Prompt injection isn't prevented.** Text from outside (an email or a ticket) can steer AI triage suggestions and summaries. Treat AI output as a suggestion; workflows shouldn't act on it unreviewed. |
| **Data at rest** | I: a stolen database or backup. | Envelope encryption (ADR-018): per-organization data keys, wrapped by the KEK. The KEK can be in Vault, Azure Key Vault or AWS KMS, where it never leaves the service. Each wrap is bound to its key id and organization. Object storage is private. | With the KEK in configuration, an operator with both the database and the configuration can decrypt. Use a key service. |
| **Audit log** | R: denying an action. T: editing history. | Every change is recorded with actor, address, request id and what changed. The log is a hash chain sealed in commit order (ADR-086). The application role can't update or delete it. `huxley admin verify-audit` checks the chain. It can be streamed to a SIEM (ADR-087), and retention never prunes it. | A database owner could rewrite the chain. Streaming to a SIEM keeps a copy out of their reach. |
| **Datasets SQL console** | E: SQL reaching other tables or writing. | A separate database login that can only be a member of a read-only role, checked at startup. Read-only transactions, statement timeouts, row caps. | — |
| **Uploads and object storage** | T: malicious files served back. I: one organization's files to another. | Downloads are always `attachment`, with a safe content type, `nosniff`, and `CSP: default-src 'none'; sandbox`. Access is checked against the record the file belongs to. Size limits. | **No malware scanning** (see the ASVS gaps). |
| **Operations** | I: secrets in logs or telemetry. D: floods. | Authorization and cookie headers are marked sensitive and kept out of traces. Secrets can come from `_FILE` variables. Rate limits on sign-in, public forms and OAuth. Request timeouts and body limits. Behind a CDN, configurable client addresses (ADR-090). | — |

## ASVS Level 2 review

Against OWASP ASVS 4.0.3, chapter by chapter. **Met** means the chapter's Level 2 requirements are covered as far as this review found. The gaps are listed so they can be scheduled; the external penetration test (Phase 13) will check this independently.

| Chapter | Status | Notes |
|---|---|---|
| V1 Architecture | Met | Documented architecture (design doc, 90+ ADRs), this threat model, centralized authorization, and encryption design. |
| V2 Authentication | Met | Argon2id, length 12+, a breached-password check against a bundled list (V2.1.7), lockout and rate limits, MFA (TOTP, WebAuthn) and MFA policy, secure recovery by emailed single-use tokens, and re-entering the current password to change it. |
| V3 Session management | Met | Random hashed tokens; `HttpOnly`, `Secure`, `SameSite`; idle and absolute timeouts; sign-out revokes the session; disabling revokes all of a user's sessions; a device list. |
| V4 Access control | Met | Deny by default; checks on every endpoint; row-level security in the database; field-level permissions; tests for denial and inheritance. |
| V5 Validation and encoding | Met | Typed request bodies, rejecting unknown fields; parameterized SQL, with dynamic lists built by `QueryBuilder` and bound values; output encoding; sanitized rich text; the parser limits and tests above. |
| V6 Cryptography | Met | Vetted libraries (RustCrypto, ring, aws-lc); AEAD for data at rest; key rotation (`rewrap-keys`); KEKs in key services. |
| V7 Errors and logging | Met | RFC 9457 errors without internals; structured logs with request ids; a tamper-evident audit log; SIEM streaming. |
| V8 Data protection | Met | Secrets encrypted and never returned; `no-store` on downloads; retention (ADR-085); redaction in run records. |
| V9 Communications | Met | TLS at the edge (Caddy, ingress, cloud load balancers); mutual TLS for runners; TLS to the database where configured (`sslmode`). |
| V10 Malicious code | Met | Signed packs with review; dependencies checked by `cargo deny`; pnpm minimum release age; Actions pinned to SHAs. |
| V11 Business logic | Met | Rate limits; optimistic concurrency on updates; approvals for sensitive workflow steps. |
| V12 Files and resources | **Partial** | Met: size limits, served as attachments, and the SSRF egress policy. **Gap: no malware scanning of uploads** (V12.4.2, applied by most to uploaded files). |
| V13 API and web services | Met | Authenticated APIs, a published OpenAPI document, CSRF protection, rate limits, and replay protection for signed webhooks (a timestamp window plus a record of signatures seen in it). |
| V14 Configuration | Met | Reproducible builds, distroless non-root images, security headers, `cargo deny`, no default credentials (bootstrap generates them). |

### Gaps to schedule

1. ~~**Breached-password check (V2.1.7).**~~ Done offline: new passwords are checked against the 12+ character entries of the million most common breached passwords (`crates/huxley-auth/data`). A full check through Have I Been Pwned's k-anonymity API (sending a 5-character hash prefix) could be added later as an opt-in setting.
2. ~~**Webhook replay (V13).**~~ Done: signatures are recorded for the replay window.
3. **Malware scanning (V12).** An optional ClamAV (or ICAP) hook on uploads, quarantining files until scanned.
4. **Prompt injection.** Mark AI output as untrusted wherever it's shown. Never let a workflow act on AI output without review by default. Keep outside text clearly separated from instructions in prompts.

### Found and fixed in this review

- **Knowledge search snippets let markup through (HTML injection).** Search results show a snippet of each article, built by PostgreSQL's `ts_headline` and displayed as HTML so matches can be bold. `ts_headline` returns the article's text as written. Its parser drops well-formed tags but passes others through: an attribute with a call in it, such as `<img src=x onerror=alert(1)>`, isn't taken for a tag. So markup in an article's body reached the staff app's and the portal's search pages as live HTML.
  - **What limited it:** the Content Security Policy (`script-src 'self'` with nonces) blocked script execution, including inline handlers. But injected elements, links and forms could still appear: in the portal, in front of clients' contacts.
  - **The fix:** snippets now mark matches with private-use characters, escape everything as HTML, then turn only those marks into `<b>`. That applies to the staff search, the portal search, and ticket suggestions.
  - **The test:** an e2e test publishes an article with such markup and checks it shows as text. It fails on the old code.

## Hostile input

Huxley parses input it doesn't control: workflow documents, OpenAPI specs that admins import, packs from uploads and registries, inbound email, expressions (some of whose data arrives from webhooks and email), and SCIM requests from identity providers. Each parser has hard limits, and tests feed each one random and malicious input to check they hold. Every parser must answer with a value or an error. None may panic, hang, exhaust memory or overflow the stack.

| Parser | Limits | Tested against |
|---|---|---|
| Workflow documents (YAML, JSON) | Size cap; YAML 1.2 rules; the YAML library's alias and depth budgets | "Billion laughs" alias bombs; 200,000-deep nesting; random and YAML-shaped text |
| OpenAPI imports | Size cap; same YAML budgets | Alias bombs; deep nesting; random text; spec-shaped documents with dangling and self-referencing `$ref`s |
| Packs (`.hxpack`) | 20 MB in total and 500 files, enforced while reading, whatever sizes the zip claims; every file hash-checked | A 200 MB zip bomb in about 200 KB; random bytes; a real pack with any single bit flipped (refused, or byte-for-byte unchanged) |
| Inbound email | Body size caps | 2,000-deep nested multiparts; random bytes; header-shaped messages |
| Expressions | Fuel (instructions), recursion depth, output size | Billion-character strings, billion-item ranges, nested loops, unbounded recursion; random and template-shaped input |
| SCIM filters and PATCH | `eq` filters only; PATCH applied to a copy, then validated | Random filters; random operations with arbitrary JSON values and paths |

The tests are the `hostile` tests in each parser's crate (`crates/huxley-*/tests/hostile.rs`, and the `random` tests in `scim_routes.rs`). They run with the rest of the test suite. Property tests run 512 random cases per check on every run, each run with fresh cases.
