# ADR-040: The client portal is a separate realm: its own accounts, sessions and API

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Client contacts use the portal (design doc §8.13) to see and raise their tickets, use the service catalog, read the knowledge base, see their assets and approve things. The design calls portal users "linked to a client contact", in "a separate auth realm (separate session cookie and domain/path) … limited to the portal API surface" (§11.1).

Mixing the two would be dangerous. Staff permissions are broad and organization-wide. A portal user must only ever see what their company allows, and staff code paths should be unable to treat a contact as staff by mistake.

## Decision

### Accounts are not users

**Portal accounts** (`portal_accounts`) are a table of their own, one per contact (`contact_id`). They are not rows in `users`.

- **Strict separation:** no staff authentication, permission check or session lookup reads portal accounts.
- **No portal tokens:** API tokens are for staff and service accounts, so a contact can't hold one.
- **The column it replaces:** `crm_contacts.portal_user_id` anticipated a `users` row and was never used. It's dropped.

**An account holds:**

- its email, normally the contact's
- a status: `invited`, `active` or `disabled`
- an optional Argon2id password hash
- MFA settings (a later PR)
- when it last signed in

**Where it belongs:** its company is the contact's primary organization. Phase 7 scopes everything to that company. Sub-organizations and sites come later, as role options.

### Sessions and cookies

**Portal sessions** (`portal_sessions`) mirror staff sessions: a hashed token, idle and absolute expiry, IP and user agent, and revocation.

- **The cookie** is `hx_portal_session`. It's `HttpOnly`, `Secure` and `SameSite=Lax`, and it's set by the portal app on the portal's own origin.
- **No crossing over:** a staff session never authenticates portal requests, and a portal session never authenticates staff ones. The two extractors read different cookies and different tables.

### Signing in

- **Magic link:** an emailed single-use link, valid for 15 minutes. It's the default, so contacts needn't keep a password.
- **Password**, when the account has one: Argon2id, with the same lockout and backoff as staff.
- **Invitations:** staff invite a contact, which emails a magic link. The first sign-in activates the account.
- **The client's own identity provider** (Entra or OIDC), matched by email domain. It signs in contacts with portal access under the provider's company, linking their identity by subject, and can give contacts with a matching address an account on first sign-in. It reuses the staff OIDC steps (discovery, PKCE, nonce, ID token checks) and starts an ordinary portal session.
- **MFA** comes in a later PR. It adds a step without changing accounts or sessions.

**Answers never reveal whether an account exists.** "We've sent a link if that address has portal access" is the reply either way.

### Portal roles

Each account has **one portal role**, from a fixed set to begin with:

| Role | Tickets | Other |
|---|---|---|
| **Client User** | Their own: see, raise, reply | Service catalog, knowledge base |
| **Client Approver** | As Client User, and approve what's sent to them | |
| **Client Admin** | Everyone's at their company | Manage their colleagues' portal access; see assets |

These map to portal permissions (`portal.ticket:read:own`, `portal.ticket:read:company`, `portal.contacts:manage`, …), checked by the portal API. When Cedar takes over evaluation (ADR-006), they become policies, and the API won't change.

### Its own API

The portal calls `/api/portal/v1/*` and nothing else.

- **Separate code:** its handlers live in their own module, with a `PortalAuthed` extractor.
- **Scope:** every query is scoped to the account's company, and narrowed further by role. Database access runs in that company's scope (RLS), so a missing filter can't leak another company's rows.
- **Hidden fields:** the API never returns internal notes, time, costs or staff-only fields.

### Managing access

- **Staff** with `crm:manage` (and the `portal` module on) invite contacts, choose their role, disable them and resend invitations. This happens on the client's contact list.
- **Client Admins** do the same for contacts of their own company, in the portal.
- **Records:** every change is audited. Actions taken in the portal are recorded with the portal account in their details. The audit actor stays `anonymous` for now, because actor ids elsewhere point at users.

## Consequences

- A second session table and a second login flow to keep secure. Both reuse the staff primitives: token hashing, Argon2id, rate limits and email tokens.
- Portal features are built twice where they touch the same data, as a staff view and a portal view. That's deliberate, because the portal view hides staff-only data by construction.
- Branding and custom domains, client IdP sign-in, MFA, and the portal features follow in separate PRs.
