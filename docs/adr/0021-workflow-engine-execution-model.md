# ADR-021: Workflow engine execution model

- **Status:** Accepted
- **Date:** 2026-11-23

## Context

ADR-002 puts the job queue in PostgreSQL; ADR-003 makes execution journal-based; ADR-020 makes control flow structured. This records how those combine: what is journaled, what a job is, and what each control-flow step outputs.

## Decision

### Storage

- `workflows` holds the editable draft; `workflow_versions` holds immutable published documents. A run pins a version (or, for test runs, a snapshot of the draft).
- `runs` and `step_runs` are **partitioned by month on their UUIDv7 id** (`runs.id`, `step_runs.run_id`). The id is time-ordered, so range partitions on it are month partitions with a plain single-column primary key, and retention drops whole partitions. A default partition catches anything outside the pre-created range.
- `step_runs` has one row per **attempt** of a step at a **step key**: the step id plus the indices of the enclosing `foreach` iterations, outermost first (`add_member[2]`, `inner[2][0]`). Step ids are unique per workflow (ADR-020), so the key is unique per run.
- `jobs` is the queue (ADR-002): claimed with `FOR UPDATE SKIP LOCKED`, leased with `locked_until` and extended by heartbeats. An expired lease makes the job claimable again, so there is no separate reaper. Workers are woken with `NOTIFY huxley_jobs`.

### Jobs

There are two job kinds:

- **advance** (per run, deduplicated while queued): locks the run row, walks the document against the journal, and makes progress. Steps without side effects (`transform`, `set_var`, `return`, `fail`) and the decisions of control-flow steps (which branch, which items) are evaluated here and journaled at once. Steps with side effects or that take time (`http.request`, `core.notify`, `wait`, `subworkflow`) get a `step_runs` row in state `running` and a **step** job.
- **step** (per step key, deduplicated): executes one step outside any transaction, then, in one transaction, records the result (or schedules a retry by inserting the next attempt and re-queueing itself with a delay) and enqueues an advance.

`wait` is a step job whose `run_at` is the wake-up time: the queue is the timer table.

### Walking

The walk is a pure function of the document and the journal. A step whose latest attempt succeeded or was skipped is passed over; one that is running or waiting makes its block pending; one that failed fails its block (unless `continue_on_error`, which records the error as its output). Decisions that must not change on resume are journaled on the control-flow step's own row: an `if`/`switch`'s chosen branch and a `foreach`'s evaluated item list. Expressions that read the clock or random numbers (`now()`, `uuid()`, `secrets.generate()`) are therefore evaluated once.

### Outputs of control-flow steps

| Step | Output |
|---|---|
| `if`, `switch` | The output of the last step of the block that ran (`null` if none ran) |
| `foreach` | A list: for each item, the output of the last step of its iteration |
| `parallel` | An object: branch id → the output of that branch's last step (`join: any`: only the winning branch) |
| `try` | The output of the last step of `steps`, or of `catch` if it caught an error |
| `subworkflow` | The child run's outputs |

### Failure, cancellation, retries

- A failure inside `foreach` stops new iterations from starting; the step fails once the iterations already running have settled. `parallel` branches are independent: each runs to the end of its own block, and the step fails (with the first error) once none is still running. With `join: any`, the first branch to succeed completes the step, the others start nothing further, and their scheduled steps are cancelled.
- `set_var` refuses values derived from secrets (run variables are stored in the clear); a `transform` that reads a secret stores its output encrypted, and later steps read the real value. The workflow's outputs can't contain secrets.
- An uncaught failure runs `spec.on_error` (with `error` in scope), then marks the run failed.
- Cancellation is cooperative: the run is marked, queued jobs for it are deleted, and in-flight steps finish (their HTTP calls are aborted where possible) but nothing new starts.
- Retries follow the step's `retry` policy; each attempt is a new `step_runs` row. A job that fails for an infrastructure reason (database down, process killed) is retried by the queue and re-executes the same attempt: execution is at-least-once, and each step gets an idempotency key derived from the run id and step key, stable across retries.

## Consequences

- Resuming after a crash needs no special code path: the next advance recomputes everything from the journal.
- One advance per run at a time (the row lock) serializes decisions without serializing step execution: `parallel` branches and `foreach` iterations run on any number of workers.
- Every decision and output is inspectable afterwards, which the run view and "resume from failed step" build on.
- Advances re-walk the document each time, which is cheap for workflows of a few hundred steps but means very large loops (thousands of iterations) walk thousands of journal entries per advance. Loop size is capped (`foreach` over at most 10 000 items) until that matters.
