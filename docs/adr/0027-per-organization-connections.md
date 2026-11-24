# ADR-027: One partner connection serves every client through organization variables

- **Status:** Accepted
- **Date:** 2026-11-23

## Context

MSPs usually reach their clients' Microsoft 365 tenants through **one** partner identity: a multi-tenant app registration with GDAP role assignments (design doc §7.2, "a parent connection plus a child org variable equals the effective connection"). For each client, only the tenant in Microsoft's token URL differs. A connection per client would mean copying the same client ID and secret dozens of times, and rotating it everywhere.

## Decision

- **A reference instead of a value:** a connection's server variable can be `{{ vars.<key> }}`. Nothing else is allowed in it: no other expressions and no surrounding text.
- **Resolved per call:** when a run calls an action (or a script step gets credentials), the reference takes the **calling run's organization's** value of that variable, with normal inheritance and locks. Values must be letters, digits, `.`, `-` and `_`, so they can't redirect the base URL or the token URL. A missing variable fails the step clearly.
- **The pattern:** make one shared connection at the MSP with `tenant: {{ vars.m365_tenant }}`, and set `m365_tenant` on each client organization.
- **Tokens:** a connection whose values depend on the caller is cached **per resolved token URL**, encrypted in its own table and locked per connection while fetching. A token for one client's tenant is never used for another's.
- **Limits:** only **client-credentials** connections, plus base URLs, can use references. A sign-in (authorization code) connection belongs to whoever signed it in, so references are refused there. Calls without a run (polls, form lookups) can't use a per-organization connection.

## Consequences

- Rotating the partner secret is one edit.
- Who may set a client's tenant variable controls where that client's calls go. Variables are already permissioned and can be locked from above.
- Partner Center's own API (customers, subscriptions) needs delegated (app + user) tokens for most operations. It isn't part of this. It will need a sign-in connection at the MSP.
