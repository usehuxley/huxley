# ADR-009: Separate SvelteKit apps for staff and portal

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Technicians and client contacts have different auth realms, themes, and attack surfaces.

## Decision

Build two SvelteKit apps, `apps/web` and `apps/portal`, sharing `packages/ui`, `packages/api-client`, and later the form and widget renderers.

## Consequences

Smaller attack surface for the portal, independent theming and deployment, and some duplicated app plumbing.
