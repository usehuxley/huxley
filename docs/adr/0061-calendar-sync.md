# ADR-061: Calendar sync reads free/busy, through connections and a pack

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

The user asked for calendar sync so that dispatching and scheduling are accurate (plan #193). Until now Huxley knew only working hours, approved time off and its own appointments. A technician in a customer meeting, booked in Outlook, still looked free.

## Decision

- **Free/busy, not events.**
  - **Microsoft 365:** Graph's `getSchedule` returns busy, tentative, out-of-office and working-elsewhere blocks for up to 20 mailboxes per call, over up to 62 days. Private events show only as busy, and there are no per-person sync tokens to keep.
  - **Everything else:** an iCalendar feed URL, such as Google Calendar's secret iCal address. It's read with recurrence expanded (`rrule`; MIT/Apache, the only new dependency being `chrono-tz`), exceptions and moved occurrences followed, and cancelled and transparent events skipped. Outlook's common Windows time zone names are mapped to IANA ones.
- **Through the platform, not beside it.** Graph is called with a new `getSchedule` action in the `msgraph` pack, using a connection chosen under Settings → Calendars. Credentials, OAuth, the egress policy, retries and run history are the same as for every integration. Feeds are fetched through the engine's egress policy, and their URLs are sealed with the instance's keys and never returned.
- **A step and a pack.** `core.calendar.sync` replaces each person's busy blocks from a day back to 60 days ahead. The built-in **Calendar sync** pack runs it every five minutes. A schedule, not push notifications: five-minute freshness is enough for dispatch, and push (Graph subscriptions) can be added later behind the same step.
- **Busy blocks** (`wf_busy_blocks`) are instance-wide, like the rest of the workforce module. Subjects are kept only when chosen, and never for private events.
- **What they change:**
  - Availability treats busy time as unavailable (`busy`) and out of office as `away`. Dispatch and fit already use availability, so they follow.
  - The dispatch board shows busy blocks.
  - Capacity subtracts busy hours from each day, capped at that day's working hours.
- **Next:**
  - writing dispatched appointments to people's calendars (opt-in), kept in step when they move or are cancelled
  - `calendar.conflict` when calendar changes clash with booked work
  - Google Workspace natively

## Consequences

- No new credential store, scheduler or HTTP client: calendar sync is an integration action, a step and a pack.
- Free/busy can't tell Huxley's own written appointments apart from other meetings. When write-back arrives, capacity must not count those twice.
