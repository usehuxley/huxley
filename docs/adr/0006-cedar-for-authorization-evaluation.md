# ADR-006: Cedar for authorization evaluation

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

RBAC must be very granular: scoped role assignments that inherit down the org tree, conditions, and field-level rules. It also has to filter list queries, not just check single records.

## Decision

Compile role assignments and conditions into Cedar policies (`cedar-policy` crate, Apache-2.0), evaluated in-process behind an internal `Authorizer` trait that offers point checks and SQL filter generation.

## Consequences

Policies are analyzable and testable. Cedar is Rust-native. The trait keeps Cedar replaceable. PostgreSQL RLS provides a second layer of org isolation (ADR-010).

## Update (2026-11-22)

The first RBAC slice ships without Cedar. With only roles × org scopes and inheritance, plain matching is simpler and easy to verify. Evaluation sits behind `Principal::can_at_path`, and Cedar replaces it there when conditions and field-level permissions are added. The decision itself stands.
