# ADR-088: SCIM provisioning hangs off sign-in providers; groups are teams

- **Status:** Accepted (implementer's design). Implements "SCIM provisioning" from Phase 13 (plan §3).
- **Date:** 2026-11-27

## Context

Organizations that sign in with Entra ID, Okta and the like want the identity provider to own the account lifecycle: create people when they're assigned, update them, and cut access when they leave, with no one doing it by hand in Huxley. SCIM 2.0 (RFC 7643/7644) is how identity providers do this.

Huxley users and teams are instance-wide (design §11). Sign-in providers (`oidc_providers`) already exist, with just-in-time creation and claim-to-role mappings.

## Decision

- **Provisioning is a feature of a sign-in provider.** Each provider can have one SCIM bearer token (`hxscim_…`). Only its SHA-256 is stored, it's shown once, and it can be replaced or removed. The token identifies the provider, so there's one base URL, `/scim/v2`, served by the API outside the `/api/v1` CSRF layer. It needs `security:policy:manage`, like the rest of SSO.
- **SCIM Users are Huxley users.** A user's SCIM `id` is their Huxley id.
  - `scim_users` links the user to the provider and keeps `userName` (unique per provider), `externalId` and the SCIM `name`.
  - **Creating** makes a password-less user with a verified email. If a human user with that email already exists, it links them instead, keeping their roles. A service account's email is refused.
  - **Updating** changes the email, display name and status.
  - **Deactivating** (`active: false`) disables the user and revokes their sessions.
  - **Deleting** disables, removes the user from the provider's teams, and unlinks. It never deletes, because the audit log and records point at users.
- **SCIM Groups are teams.** `scim_groups` links a team to one provider.
  - Creating a group adopts an unmanaged team with the same name, so pre-built teams and their roles carry over.
  - Membership becomes exactly what the provider sends: it's the source of truth for teams it manages.
  - Deleting the group deletes the team.
- **Roles aren't provisioned.** Admins give roles to teams in Huxley, and claim mappings still apply at sign-in. SCIM has no standard for authorization, and every provider's custom extension differs.
- **Protocol support** covers what identity providers send:
  - `eq` filters on the lookup attributes, including `emails[type eq "work"].value`.
  - `startIndex`/`count` paging, with `excludedAttributes=members` and `attributes`.
  - PUT, and PATCH with or without paths. That includes filtered paths such as `members[value eq "…"]` and `emails[type eq "work"].value`, Entra's string booleans, and extension attributes, which are ignored.
  - `ServiceProviderConfig`, `ResourceTypes` and `Schemas`.
  - Not supported: Bulk, sort, ETags and password changes.
- **PATCH is applied to the resource's JSON, then saved like a PUT**, in one transaction. A single code path enforces uniqueness and side effects.
- **Changes are audited.** The actor is the system, by way of the API, and the details record the provider.

## Consequences

- Offboarding in the identity provider ends Huxley access within its provisioning cycle (Entra: about 40 minutes, or on demand), not at the next sign-in.
- People added to a provisioned team by hand are removed at the next sync. The docs say so.
- Group membership changes answer 204 without the member list, which keeps large groups cheap.
- Deactivated users keep their team memberships, so reactivating restores access as it was. Deleted users lose the provider's teams.
