# ADR-049: Skill gaps are computed from requirements, not guessed

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 8 lists "skill-gap insights (unqualified work, expiring certifications)" among the AI features. Everything needed is already recorded:

- what queues and ticket types require (ADR-048)
- who holds which skills and certifications, and when those expire
- who worked which tickets

A model would add cost and uncertainty to questions with exact answers.

## Decision

`huxley-workforce::insights` computes, system-wide:

- **Coverage:** for each queue and ticket type with requirements, who meets all of them, out of its pool (a queue's members, or everyone for a ticket type).
- **Unqualified work:** for the last 200 tickets resolved in 90 days, the requirements each assignee didn't meet (the queue's, type's and ticket's own). These are tallied per requirement, with who.
- **Expiring certifications:** those expiring within N days (default 60), with the work their holder is the only qualified person for.
- **A summary**, one line per finding, most serious first:
  1. work nobody is qualified for
  2. expiries that would leave work uncovered
  3. single points of failure
  4. the top unqualified work
  5. other expiries

These are shown in three places:

- `GET /api/v1/workforce/insights` (`workforce:manage`)
- the People page's **Skill gaps** tab
- the `core.workforce.insights` step, which the built-in **Skill gaps** pack runs every Monday, notifying administrators when there's anything to act on

## Consequences

- Findings are exact and explainable, and they cost nothing to compute.
- The AI-assisted part of the plan (suggesting training, or drafting a hiring note) can build on these findings later as a workflow step reading the step's output.
- Unqualified work looks at a bounded window (200 tickets) to keep the weekly run cheap on large desks.
