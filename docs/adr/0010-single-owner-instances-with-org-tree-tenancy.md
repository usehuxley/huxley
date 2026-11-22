# ADR-010: Single-owner instances with org-tree tenancy

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Huxley is not sold as multi-tenant SaaS. One instance serves one owning company, which may own several MSPs.

## Decision

No tenant layer. Multiple MSPs are `provider` orgs under the owner's root org. Every org-owned row carries `org_id`, and PostgreSQL RLS (`huxley_org_in_scope`) enforces org scope as defense in depth. The scope is set per transaction and denies by default.

## Consequences

A simpler schema. Per-MSP separation comes from settings inheritance and RBAC scoped to each MSP's subtree.
