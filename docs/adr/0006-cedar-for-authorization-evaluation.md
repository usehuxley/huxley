# ADR-006: Cedar for authorization evaluation

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

RBAC must be very granular: scoped role assignments that inherit down the org tree, conditions, and field-level rules. It also has to filter list queries, not just check single records.

## Decision

Compile role assignments and conditions into Cedar policies (`cedar-policy` crate, Apache-2.0), evaluated in-process behind an internal `Authorizer` trait that offers point checks and SQL filter generation.

## Consequences

Policies are analyzable and testable. Cedar is Rust-native. The trait keeps Cedar replaceable. PostgreSQL RLS provides a second layer of org isolation (ADR-010).
