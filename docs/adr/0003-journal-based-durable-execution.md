# ADR-003: Journal-based durable execution

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Workflow runs must survive crashes and deploys without losing or silently repeating side effects. Deterministic-replay engines (Temporal-style) constrain how workflow code is written, which does not fit a visual tool.

## Decision

Commit each step's result (output or error) to `step_runs` before scheduling the next step. Resuming walks the graph, skips completed steps, and re-executes only in-flight ones. Steps receive deterministic idempotency keys.

## Consequences

Execution is at-least-once per step. Every step's input and output can be inspected in the UI. Integrations should honor idempotency keys where their APIs allow.
