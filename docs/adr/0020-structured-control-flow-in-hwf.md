# ADR-020: Structured control flow in HWF

- **Status:** Accepted
- **Date:** 2026-11-23

## Context

The design document (§6.2) says steps run in order by default and that "explicit edges (`next`, `branches`) build arbitrary DAGs", alongside control-flow nodes (`if`, `switch`, `foreach`, `parallel`/`join`, `try`/`catch`/`finally`). Supporting both free-form edges and structured nodes means two ways to express the same flow, and free-form edges raise questions the engine has to answer at runtime: what a join waits for when an upstream branch was skipped, what "the step after a loop" is, which `catch` covers a step reachable from two places, and how a step inside a loop is identified on resume.

The engine is journal-based (ADR-003): resuming walks the definition and skips completed steps. That walk is simplest and safest when every step has exactly one place in the document.

## Decision

HWF v1 has **structured control flow only**. A block is an ordered list of steps; control-flow steps own nested blocks (`then`/`else`, `cases[].steps`/`default`, `foreach.steps`, `parallel.branches[].steps`, `try.steps`/`catch`/`finally`). There are no `next` or free-form edges.

- Step ids are unique across the whole workflow, so `steps.<id>.output` is unambiguous wherever it is read.
- A step's position (its id plus loop indices, e.g. `add_group[2]`) is its journal key and the basis of its idempotency key.
- `return` and `fail` end the run; steps after them in the same block are reported as unreachable.
- Fan-in is the end of a `parallel` step (`join: all | any`), not an edge into a shared node.

This is the model used by Azure Logic Apps and Power Automate (scopes, conditions, loops), which run durably at large scale, and by most workflow-as-code tools.

## Consequences

- The engine never has to reason about graph reachability, join readiness for skipped branches, or cycles. Resume is a deterministic walk.
- The canvas draws nested blocks as containers (an `if` node with two lanes, a loop frame) rather than letting people draw arbitrary arrows. Flows that would need a goto, such as jumping back to an earlier step, are expressed with loops or sub-workflows.
- Adding edges later remains possible as a v2 feature; adding structure to an edge-based v1 would not have been.
