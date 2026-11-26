# ADR-044: Client health is a weighted average of the factors that have data

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Account managers want early warning that a client is unhappy (design doc §8.9): a score, its trend, and the reasons, plus an event to act on. The signals are uneven. A small client might have no survey answers, and a client without agreements has no renewal to watch. A missing signal must not read as a bad one.

## Decision

**Each factor scores 0–100, or nothing without enough data:**

| Factor | Score | Needs |
|---|---|---|
| Satisfaction surveys | Satisfied %, last 90 days | 3 answers |
| Net promoter score | (NPS + 100) / 2, last 180 days | 3 answers |
| SLA targets met | 100 × (1 − 2 × miss rate), last 90 days | 5 tickets with SLAs |
| Tickets staying resolved | 100 × (1 − 3 × reopen rate), last 90 days | 5 resolved |
| Ticket volume | 100 up to 1.25× the usual 30 days; 0 at 3× | 3 tickets a month, usually |
| Agreement renewals | 40 if one ends within 60 days without auto-renewal, else 100 | an active agreement |

**The score** is the weighted average of the factors that have data. Weights are 0–10 per factor, set at an organization for its clients and those below (the nearest set up the tree applies, else the defaults 3/2/3/2/1/1). **The reasons** are the factors under 60, worst first, each with what it's based on.

**Snapshots, daily.**

- `core.health.compute` (run by the **Client health** pack each morning) stores each client company's score, factors and reasons for the day. Computing again the same day replaces that day's snapshot.
- A drop of 10 points or more from the snapshot a week before emits `client.health_declining`, with the score, the previous score and the reasons. The pack tells administrators.

## Consequences

- Scores are explainable: every factor shows its number and its basis.
- Sentiment (Phase 8's AI work) and overdue invoices (Phase 10) join as factors when they exist, with their own weights. The model is the factor list plus weights, so adding one doesn't change the others.
- With no factors having data, a client has no score ("Not enough data") rather than a misleading one.
