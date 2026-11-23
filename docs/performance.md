# Performance

## Workflow engine throughput

The Phase 2 target is a sustained 100 steps per second on a modest single
node (docs/IMPLEMENTATION_PLAN.md). Measured with the load test in
`crates/huxley-server/tests/load.rs`:

```sh
HUXLEY_TEST_DATABASE_URL=postgres://… \
  cargo test --release -p huxley-server --test load -- --ignored --nocapture
```

It creates a throwaway database, starts one `huxley serve --roles worker`
process (16 jobs at once), and starts 40 runs of a workflow whose `foreach`
runs 50 iterations, 10 at a time, each scheduling a step job (a 1 ms `wait`,
the cheapest step with a side effect path) and evaluating a `transform`.
That is 101 journaled steps and 50 step jobs per run, plus the advances
that walk each run: every step goes through the full durable path (journal
writes, job enqueue, claim, heartbeat, result, re-walk).

| Date | Machine | Worker CPU | Result |
|---|---|---|---|
| 2026-11-23 | AMD Ryzen 7 9800X3D, 32 GB, PostgreSQL 18 in Docker on the same host | 16 threads | 4,040 steps in 2.6 s: **~1,550 steps/s** |
| 2026-11-23 | Same, test and worker pinned to 2 cores (`taskset -c 0-1`; PostgreSQL unpinned) | 2 cores | 4,040 steps in 2.7 s: **~1,500 steps/s** |

PostgreSQL shares the host, so these numbers include the database's own CPU
use; pinning the worker to two cores barely changes the result, so at this
scale the database, not the worker, sets the pace. Throughput scales with worker processes until PostgreSQL saturates:
workers share nothing but the queue.

### Where the time goes

- Each advance re-walks the run against its journal (ADR-021), so very
  long runs cost more per step than short ones. `foreach` is capped at
  10,000 items.
- Steps without side effects (`transform`, `set_var`, control-flow
  decisions) are evaluated inside the advance and cost no job round trip.

## Chaos

`crates/huxley-server/tests/chaos.rs` runs in CI: three worker processes
are SIGKILLed and restarted every 300 ms while eight runs execute. Every run
must finish with the right outputs, and every step key must be journaled as
succeeded exactly once.
