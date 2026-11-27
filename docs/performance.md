# Performance

## Reference sizes

Measured on 2026-11-27 with `scripts/bench.sh small|medium|large`, on one machine: AMD Ryzen 7 9800X3D (16 threads), 32 GB, PostgreSQL 18 with pgvector. Each size gets its own resources, so the parts don't compete:

- PostgreSQL runs in a container limited to its cores and memory, tuned for that memory.
- Huxley's processes are pinned to separate cores (`taskset`).
- The load generator runs on the remaining cores.

| Size | PostgreSQL | Huxley | Data | Load |
|---|---|---|---|---|
| **Small** (one modest VM) | 2 cores, 4 GB | 2 cores: 1 API process, 1 worker | 25 client orgs, 10 technicians, 20,000 tickets | 10 technicians at once |
| **Medium** | 4 cores, 8 GB | 4 cores: 2 API processes, 2 workers | 150 client orgs, 50 technicians, 150,000 tickets | 50 at once |
| **Large** | 6 cores, 12 GB | 6 cores: 4 API processes, 4 workers | 500 client orgs, 200 technicians, 600,000 tickets | 150 at once |

The large size is as large as this machine can run while still keeping the load generator on cores of its own. Real deployments put the database, the API, workers and load on separate machines, and scale further with more of each (see [Kubernetes](./kubernetes.md)).

The data is seeded as a year of history: a fifth of the tickets are open and the rest resolved or closed, with two notes each. Technicians hold the built-in Technician role at the provider, inherited by every client, so every request runs the real permission and row-security checks.

### The API under a technician's workload

Every simulated technician repeats this mix, without pause, for 60 seconds after a 10-second warm-up:

- list open tickets (25%) and their own tickets (15%);
- open a ticket (25%) and read its notes (15%);
- My work (10%);
- add a note (7%) and create a ticket (3%).

| | Small | Medium | Large |
|---|---|---|---|
| **Requests per second** | **924** | **1,605** | **1,390** |
| Median / p95 / p99, all requests | 10 / 22 / 27 ms | 29 / 55 / 63 ms | 98 / 172 / 192 ms |
| List open tickets (median / p95) | 15 / 25 ms | 44 / 56 ms | 150 / 181 ms |
| List my tickets | 14 / 23 ms | 33 / 62 ms | 105 / 189 ms |
| Open a ticket | 6 / 14 ms | 19 / 29 ms | 78 / 104 ms |
| Read a ticket's notes | 6 / 14 ms | 18 / 28 ms | 77 / 102 ms |
| My work | 9 / 19 ms | 34 / 57 ms | 115 / 161 ms |
| Add a note | 11 / 21 ms | 34 / 49 ms | 107 / 136 ms |
| Create a ticket | 15 / 26 ms | 44 / 60 ms | 129 / 164 ms |
| Errors | 0 | 0 | 0 |

Every simulated technician here acts continuously. A real technician makes a request every few seconds, so even the small size serves far more people than the 10 measured. At the large size, the 150 simulated users keep 12 cores busy and requests queue for them: latency there measures that queue, not slow queries.

### What the benchmark found

The first runs found ticket lists that slowed as tickets grew. The open-ticket list's median was **651 ms with 20,000 tickets** and **13.6 s with 600,000**, and at the large size requests failed. There were three causes, all fixed:

- **Optional filters written as `($1 IS NULL OR …)`.** After a few executions, PostgreSQL reuses a generic plan for prepared statements. These filters gave that plan nothing to work with, so it joined every ticket to its organization, type and status before filtering. Lists now send only the filters in use.
- **Joining before limiting.** The list picked its 50 tickets, but the planner still joined all of them to their names and SLAs first. Lists now choose the page from the tickets alone, using new indexes on priority and age for open tickets, then fetch those 50 by id. My work does the same.
- **Counting every match.** The "N tickets" count read every open ticket on every page. It now stops at 10,000, shown as "10,000+".

| Requests per second | Small | Medium | Large |
|---|---|---|---|
| Before | 58 | 103 | 25 (34 errors) |
| After | 924 | 1,605 | 1,390 |

### Workflow engine at the reference sizes

The engine test below, with more runs and more worker processes for the larger sizes:

| Size | Runs | Workers | Steps per second |
|---|---|---|---|
| Small | 40 | 1 | 1,228 |
| Medium | 80 | 2 | 1,407 |
| Large | 160 | 4 | 1,427 |

At these sizes PostgreSQL, not the number of workers, sets the pace. Every step is a durable write, so adding workers stops helping once the database's commit rate is the limit.

### Running it

```sh
scripts/bench.sh small     # or medium, large
```

It starts and removes its own PostgreSQL container, builds the benchmarks (a few at a time, to spare memory), and prints the tables above. To look at query plans afterwards, run the API benchmark directly against a database you keep, with `BENCH_KEEP=1`. It then leaves its database behind and prints its URL (see the comment at the top of `crates/huxley-server/tests/api_bench.rs`).

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
