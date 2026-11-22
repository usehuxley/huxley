# ADR-017: Everything outside the core platform is a toggleable feature module

- **Status:** Accepted
- **Date:** 2026-11-22

## Context

Some MSPs want Huxley only as an automation engine and have no use for ticketing, sales, or billing. Forcing every module on clutters the product for them and enlarges their attack surface.

## Decision

The core platform (orgs, users, RBAC, audit, the automation engine, integrations, datasets, packs, AI/MCP, notifications) is always on. Everything else is a feature module registered through a module registry that gates its UI, API routes, permissions, `core.*` workflow actions, system workflows, and report data sources. Modules declare dependencies. They can be toggled instance-wide or per provider org. Turning a module off keeps its data. Bootstrap offers presets: *Automation only*, *Service desk*, *Full PSA*. See design doc §8.0.

## Consequences

Automation-only deployments are first-class. Every module must work through the registry and be tested with itself turned off, which constrains how modules depend on each other (only through declared dependencies and domain events).
