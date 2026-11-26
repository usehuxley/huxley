# ADR-050: Ticket insights: an AI summary by workflow, similar tickets by embedding

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 8's ticket insights are a summary, entities, similar tickets, and KB and Action suggestions. Knowledge-base suggestions and Actions already show on tickets. What's missing is a quick catch-up for a technician picking up a long ticket, and a way to find tickets about the same thing.

## Decision

**The summary is a workflow.** The built-in **AI ticket insights** pack's *Summarize tickets with AI* runs on `ticket.note_added`, and on `ticket.insight_requested` (the ticket page's Refresh, `POST /api/v1/tickets/{id}/insight/refresh`).

- It reads the title, description and last 20 notes, up to 12,000 characters.
- `ai.extract` returns:
  - a summary of at most 60 words
  - people, devices, applications and errors
  - a single next step
- **`core.ticket.insight`** stores it in `sd_ticket_insights`, one row per ticket. Entity kinds are fixed, and each list is cleaned: distinct, short, and at most 10.

The prompt, triggers and model belong to the organization like any workflow, and every call is in the AI log.

**Similar tickets use the ticket's own embedding**, which the search-by-meaning pack already stores:

- `GET /api/v1/tickets/{id}/similar` finds the nearest tickets by the same model, among those the caller can read, with no AI call.
- Without pgvector, or before the ticket is embedded, it says why instead.

## Consequences

- Insights cost one AI call per note (or per Refresh). An organization that finds that too much can change the trigger: a filter on note kind, or Refresh only.
- Summaries are a convenience, not a record: they're replaced each time and never sent to clients.
- Similar tickets are only as good as the embeddings, and they appear once the embedding pack has caught up.
