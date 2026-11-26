# ADR-055: Problems extend problem tickets; incidents link to them

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 9's problem management covers problem records, linking incidents, known errors, root cause, and getting a workaround into the KB. As with changes (ADR-054), Huxley already has a `problem` ticket type, and tickets already link to each other.

## Decision

- **Problem details** live in `sd_problems`, one row per problem ticket:
  - root cause and workaround
  - known error: it can be set only with both a root cause and a workaround, and records when it was set
  - the article drafted from it
- **Becoming a known error** raises `problem.known_error`, with the root cause, workaround and count of linked incidents. Workflows can tell the linked incidents' assignees, reply to requesters, or open a change.
- **Incidents link with two new link kinds** that are each other's inverse: an incident's `problem` (its cause) and a problem's `incident`s. Everything links already do still applies (both directions, activity, permissions), so no new linking API is needed.
- **Workaround → KB:**
  - `POST /tickets/{id}/problem/article` (`kb:write` too) drafts "Known error: <title>" with symptoms, cause and workaround sections, tagged `known-error`.
  - The draft is **submitted for review** and linked to the problem, through the knowledge base's usual review.

## Consequences

- Problems are tickets, so queues, SLAs, assignment, notes and reporting work on them unchanged.
- The `problem.known_error` event is where automation hooks in: the platform doesn't decide what happens to incidents.
- Resolving a problem doesn't resolve its incidents automatically. A workflow can, through `ticket.status_changed` and the links.
