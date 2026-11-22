# ADR-011: System automations ship as packs of workflows

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Modules need automation (mailbox polling, SLA escalation, AI triage). Hidden background code would make the platform hard to customize.

## Decision

Module automations are system workflows shipped in built-in packs. They can be viewed, copied, and overridden per org. Upgrades ship new versions.

## Consequences

MSPs change platform behavior without forking. The engine must be capable enough for every module's needs.
