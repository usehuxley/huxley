# ADR-033: A run records each `lookup()` answer the first time it asks

- **Status:** Accepted (implementer's design, per the maintainer's "you choose")
- **Date:** 2026-11-25

## Context

The design calls for a `lookup(dataset, …)` helper in expressions (§6.3, §9). Two facts make that harder than it looks:

- **Expressions are synchronous.** The expression crate has no database access.
- **Expressions are re-evaluated on replay** (ADR-021). Each time a run advances, the walk re-evaluates `when` conditions, `if` conditions, `switch` values and step inputs against the journal.

A lookup that read the database during evaluation could answer differently on a later replay. The run would then take a different branch from the one it already took. That breaks the guarantee that a run's history is consistent.

## Decision

- **A run records each answer.** In `run_lookups`, keyed by the dataset and the filters, it records the first answer it gets: the row, no match, or a failure. Every later evaluation in that run uses the recorded answer.
- **A missing answer parks the step, it doesn't fail it.** If an expression asks for a lookup that isn't recorded yet, the expression engine notes it and stops. The walker treats that as "waiting" and records nothing for the step. The engine fetches the missing answers through the same path as `dataset.query`: the run's organization's dataset (or the nearest above), and rows of its subtree only, enforced by row-level security. It records them and walks again straight away, in the same advance.
- **Limits:** at most 50 new lookups per walk, and 8 rounds per advance. Beyond that, the run continues in its next advance.
- **Failures** (an unknown dataset, say) fail the step. Resuming the run forgets recorded failures, so they're asked again.
- **Outside runs,** `lookup` is a global that says it's available only while a workflow runs. **Preview with the last run** uses the answers that run recorded.

## Consequences

- Runs stay deterministic. The trade-off is that a long-running run keeps seeing a row as it was when it first asked. The docs say to use a `dataset.query` step to read a row as it is now.
- Recorded rows are client data. They follow the run's organization under row-level security, like `step_runs`, and are deleted with the run.
