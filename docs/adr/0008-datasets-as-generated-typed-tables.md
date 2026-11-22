# ADR-008: Datasets as generated typed tables

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Users build datasets from integration data and query them in reports, forms, and workflows. JSONB-only storage makes indexing and query performance hard.

## Decision

Each dataset gets a managed, typed table in the `datasets` schema with generated DDL and migrations when its schema changes. Queries go through a structured query spec compiled to parameterized SQL, never raw user SQL (except an optional, read-only admin console).

## Consequences

Fast, indexable queries that fit the semantic layer. Schema changes need managed migrations.
