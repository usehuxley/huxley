# ADR-004: HWF as the single canonical workflow format

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Workflows can be built on the canvas, written as code, and created through MCP. Separate representations would drift apart.

## Decision

All three edit one versioned document format, the Huxley Workflow Format (HWF), stored as JSON and authored as YAML. Canvas layout lives in a separate `ui` block. The JSON Schema is published at `usehuxley.io/schemas/hwf/v1.json`.

## Consequences

Canvas, code, and MCP stay interchangeable, and Git diffs are meaningful. HWF changes need compatibility discipline once v1 is frozen (Phase 7).
