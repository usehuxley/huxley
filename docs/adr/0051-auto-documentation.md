# ADR-051: Auto-documentation drafts; people decide

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 8 lists auto-documentation: resolution notes, KB drafts into review, and draft time entries for timesheets. Each touches a record people rely on: the ticket's history, the published knowledge base, and billable time. An AI mistake in any of them is costly if it lands as fact.

## Decision

The built-in **AI auto-documentation** pack's *Document resolved tickets with AI* runs on `ticket.status_changed` to a resolved status. One `ai.extract` call over the ticket returns:

- a resolution note (at most 80 words)
- whether the fix is worth sharing, with an article title and body if so
- an estimate of the hands-on minutes

Every output is a draft that a person confirms:

- **Resolution note:** an *internal* note on the ticket, labelled "Resolution summary (AI draft)". Clients never see it, and it doesn't replace the resolution people write.
- **Knowledge article:** `core.kb.draft` creates it tagged `ai-draft`, **submits it for review**, and links it to the ticket. It's published only through the existing review.
- **Time:** `core.time.suggest` writes a **suggestion** (`sd_time_suggestions`), not a time entry, and only when the assignee logged no time on the ticket. Suggestions don't count anywhere: not timesheets, billing or coverage. On **My time** the person adjusts the minutes, picks a work type and logs it (a real entry, through the usual checks, including locked timesheets), or dismisses it.

## Consequences

- Nothing the AI writes becomes a record people rely on without a person's action.
- One AI call per resolution. Organizations can narrow the trigger (for example by queue) or drop steps, since the pack is an ordinary workflow.
- Time suggestions stay separate from entries, so the timesheet, billing and agreement code is unchanged.
