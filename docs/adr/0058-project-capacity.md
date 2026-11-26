# ADR-058: Capacity is computed from schedules, appointments and task estimates

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 9 asks for resource capacity planning shared with the dispatch calendar, matching task skill requirements to people (§8.14, §8.15). Huxley already has working schedules and approved time off (workforce), appointments (the dispatch calendar), skill requirements on tickets, and `fit`, which ranks people against requirements.

## Decision

- **Capacity is computed on request, not stored.** A person's hours in a week are their schedule's hours on each day (Monday to Friday, 8 hours, without a schedule), less days of approved time off.
- **Booked** time is:
  - appointments in the week
  - plus the remaining estimate (estimate less time logged) of each of their open tasks in planned or active projects.
- **Spreading a task:** its remaining estimate is spread evenly over the person's working days from its start to its due date. A task with only one of the dates puts it all on that day. Work dated before the first week is still owed, so it counts in the first week. Tasks without dates are reported as **undated**, not placed in a week.
- A week is **over** when booked hours exceed capacity.
- **Skills:** template tasks can name skills at a level. They become the task ticket's requirements, so `GET /workforce/fit?ticketId=` ranks people for a task. The project page suggests, for each unassigned task, up to three people who are qualified and available, by skill match and then by free hours this week, with one click to assign.
- **Calendar sync** (planned for Phase 9) will subtract busy calendar time from capacity in the same place.

## Consequences

- No new tables, and nothing to keep in step: capacity always reflects the current schedules, estimates and time logged.
- Spreading evenly is an approximation. It's enough to warn about over-booking, not to plan by the hour.
