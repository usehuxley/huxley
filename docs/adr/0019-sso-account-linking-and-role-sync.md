# ADR-019: SSO account linking, role sync, and MFA trust

- **Status:** Accepted
- **Date:** 2026-11-22

## Context

OIDC single sign-on (design doc §11.2) lets an outside system vouch for who someone is. Done carelessly it lets any account at a provider sign in as anyone: Google will issue a token for any address its users claim, and an unrestricted Entra app accepts users from every tenant. Role mapping and the MFA policy (PR #7) also have to interact sensibly with SSO.

## Decision

- **Identity key.** A linked identity is (provider, subject). Email is only used for the first link.
- **Domains.** Every provider must list allowed email domains. Sign-in with any other address is refused, even for already-linked identities.
- **Linking and JIT.** A first sign-in links to the existing user with that email, or creates one when JIT provisioning is on, only if the email is verified: `email_verified: true`, or `trust_email` for providers that don't send it. `trust_email` defaults on only for the Entra preset, which is single-tenant: the tenant's admins control its addresses. Multi-tenant Entra (`common`/`organizations`) is refused until it can validate the `tid` claim against an allow-list.
- **Roles.** Claim values (Entra app roles by default, groups for Okta/Authentik/Keycloak) map to role + org assignments. Each sign-in adds missing mapped assignments and removes stale ones, touching only assignments the provider created (`role_assignments.oidc_provider_id`). Hand-made assignments are never removed. Owner can't be mapped. Deleting a provider removes what it granted.
- **MFA.** Sessions record `auth_method` and `mfa`. An SSO session satisfies the MFA policy per provider: when the ID token's `amr` includes `mfa` (default), always, or never.
- **Flow.** Authorization code with PKCE (S256) and a nonce. The state is a random token whose hash is stored server-side, single use, and expires after 10 minutes. The web app binds it to the browser with an HttpOnly `SameSite=Lax` cookie and checks it before calling the API, which prevents login CSRF. The redirect URI is `{auth.public_url}/login/oidc/callback`. Discovery is cached for 10 minutes, and the HTTP client follows no redirects.
- Client secrets are envelope-encrypted with the root org's key and are write-only.

## Consequences

SSO users created by JIT have no password, so they can't yet re-authenticate to add a passkey or TOTP; they rely on their provider's MFA. Entra group overage (more than 200 groups, sent as a `_claim_names` reference) isn't resolved; app roles avoid it. SCIM provisioning and multi-tenant Entra come later.
