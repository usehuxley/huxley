# ADR-037: SLAs as per-ticket clocks over a snapshot calendar

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Service desk tickets need SLAs (design doc §8.2):

- **Targets:** how long until the first response, and until resolution, by priority.
- **Business hours:** both are counted in business hours, from the client's site calendar.
- **Pausing:** the resolution clock stops while the ticket waits on the client or a third party.
- **Alerts:** warnings before a target is missed, and breaches when it is. Workflows can act on both.

## Decision

### Policies

An **SLA policy** (`sd_sla_policies`) has a target per priority (`sd_sla_targets`): response and resolution minutes. Either can be left out.

Its **hours** are one of:

- `client_site` (the default): the client's primary site hours and timezone. If that site has no hours, the policy's own hours apply.
- `policy`: the policy's hours, in the policy's timezone. Without one, it uses the organization's `general.timezone` setting.
- `always`: around the clock.

**Which policy applies:**

- The nearest assignment (`sd_sla_assignments`) on the ticket's organization or an organization above it.
- Otherwise the default policy. A seeded *Standard* policy is the default.

### Each ticket's SLA

When a ticket is created, it gets a row in `sd_ticket_sla` with a **snapshot**: the policy, its targets and the resolved calendar. Changing a policy affects new tickets. Existing tickets keep the terms they were opened under.

The ticket's priority picks the target, and changing the priority re-targets the ticket.

**The response clock:**

- It runs in business time from creation.
- The first public note from a person meets it.
- It doesn't pause. A waiting status still owes the client an answer.

**The resolution clock:**

- It accrues business seconds while the ticket is in a status that doesn't pause the SLA and isn't resolved or closed.
- It stops when the ticket is resolved, and resumes if the ticket is reopened.
- The row keeps the seconds accrued so far and when the clock last started (`resolution_clock_at`). Every change to the ticket brings the clock up to date.

**Due times:**

- Each change also recomputes the due times: the time the remaining business seconds run out.
- A warning time is computed at 75% of the target.
- While the clock is paused, there's no due time.

### Warnings and breaches

The scheduler leader checks once per tick (5 seconds):

- **Warnings:** a clock passed its warning time and its target isn't met yet.
- **Breaches:**
  - A response is breached if it's past due, or came after its due time.
  - A resolution is breached if the clock is running past due, or the ticket was resolved after its target.

Each warning and breach is marked once, added to the ticket's timeline, and emits `ticket.sla_warning` or `ticket.sla_breached`, with which clock it was.

It skips organizations where the module is off.

### Configuration

Managing policies and assignments needs `servicedesk:configure`. A policy can't be archived while it's the default.

## Consequences

- **The snapshot keeps the maths stable:** a ticket's due times don't move because someone edited a site's hours or a policy.
- **What's left out for now:** holidays, per-type or per-queue policies, and pausing the response clock. Each would be another input to the policy lookup or the calendar.
