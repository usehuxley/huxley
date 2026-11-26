# ADR-062: Appointments are written to calendars by an event-driven workflow

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

ADR-061 reads busy time. The plan's exit criterion also asks that a booked appointment appear in the technician's Outlook calendar and move with it, and that clashes are flagged.

## Decision

- **Appointments raise events:** `appointment.booked`, `.moved` (with `previousUserId` when the person changes) and `.cancelled`, at the ticket's organization.
- **A step writes them.** `core.calendar.write` (the appointment's id):
  - creates the event with Graph (idempotently: the appointment's id is its `transactionId`), updates it when the appointment moves, deletes it from the old calendar and creates it in the new one when it changes hands, and deletes it when the appointment is cancelled
  - writes it again if someone deleted it by hand
  - Written events are recorded in `wf_calendar_writes`, with no foreign key: a cancelled appointment's event still has to be found to remove it.
- **The Calendar sync pack** (1.1.0) adds *Put appointments in calendars*, triggered by those events. Writing back is opt-in (`write_back` in the calendar settings), because the connection then needs `Calendars.ReadWrite`.
- **No double counting.** Free/busy can't tell Huxley's own events from other meetings, so blocks matching a written event (same person, same start and end) are left out of the dispatch board, capacity and conflict checks: the appointment already counts.
- **Conflicts:** each sync finds busy or out-of-office blocks overlapping someone's upcoming appointments and raises `calendar.conflict` once per appointment and block (`wf_calendar_conflicts`). What happens next is up to workflows.
- **Graph actions:** `createEvent`, `updateEvent` and `deleteEvent` join the `msgraph` pack (1.3.0).

## Consequences

- Booking, moving, reassigning and cancelling all reach the calendar through the same audited, retried workflow runs as everything else.
- A clash is reported once. If the appointment moves, its new times are checked afresh.
