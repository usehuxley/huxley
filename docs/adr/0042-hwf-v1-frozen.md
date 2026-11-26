# ADR-042: HWF v1 is frozen; later changes only add

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

HWF (`apiVersion: usehuxley.io/v1`) has been used for real by the built-in packs behind email intake, dispatch, notifications, SLAs, the CMDB and timesheets (Phases 5–6). People now keep workflows in Git, and the TypeScript SDK (ADR-016) compiles to HWF. Both need a stable target: a document that works today must keep working.

## Decision

**v1 is frozen.** Within `usehuxley.io/v1`, changes may only add:

- new step types and trigger types
- new optional fields, with defaults that keep today's behaviour
- new allowed values (enum members)
- looser rules (accepting more)

**Not allowed in v1:**

- removing or renaming a step type, trigger, field or allowed value
- making a field required, or tightening what a field accepts
- changing what an existing field means, or its default

A change like that needs `usehuxley.io/v2`. The server would read both for at least two major releases, and `huxley workflows upgrade` would rewrite v1 files as v2.

**Enforced by a test.** `schemas/hwf/v1.baseline.json` is the schema as frozen. `crates/huxley-hwf/tests/compatibility.rs` compares the published schema with it and fails on any removal, any newly required field, any tightened type, or anything that no longer accepts extra fields. After an additive change, the baseline is refreshed (`UPDATE_BASELINE=1`) so the next change is compared with it. A reviewer should see a baseline change only alongside an addition.

**Expressions** (`{{ … }}`, Jinja via MiniJinja) follow the same rule. New filters, functions and context keys may be added. Existing ones keep their names and meaning.

## Consequences

- Workflows in Git, packs and SDK builds keep working as the server is upgraded.
- Some things will be awkward to fix within v1 and have to wait for v2. That is the price of the guarantee.
- The SDK's types are generated from the published schema, so they only ever grow too.
