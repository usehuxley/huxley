# ADR-043: Satisfaction surveys are sent by a workflow step and answered by link or in the portal

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

CSAT (design doc §8.9) asks requesters how it went when a ticket is resolved. It needs sampling and per-company opt-out, one-click answers from email, and answers in the client portal. The results feed dashboards and alerts. MSPs differ in when they want to ask, and whether to skip some ticket types, queues or clients.

## Decision

**A step, run by a built-in pack.**

- `core.csat.survey` (the `cx` module) starts a survey for a resolved ticket and emails it.
- It skips the ticket when its company opted out (`csat.enabled`), there's no requester, they were asked about it in the last week, or sampling (`csat.sample_percent`) leaves it out. Its output says which.
- The built-in **Satisfaction surveys** pack runs it on `ticket.status_changed` to resolved, and tells the assignee about poor ratings.
- MSPs change when to ask by changing the workflow, e.g. with a filter on the queue or type, or a delay.

**Settings follow the organization tree:** on or off, the sample percentage, and thumbs or 1–5 (`csat.thumbs`). The answer is fixed per survey (its `scale`).

**Answering:**

- **The email** has a link per rating, carrying a single-use token. The link opens a page with that rating chosen, and the answer is recorded when they press **Send**. Mail scanners open links, so a click alone doesn't record anything.
- **In the portal,** the requester answers on the ticket.
- **Rules:** one answer per survey, within 30 days.
- **Events:** `csat.responded` is emitted with the rating, whether it's positive (4–5, or thumbs up), the comment and the assignee.

**Reporting:** the `csat` analytics source has a `satisfaction` field: 100 when positive, 0 when not, empty when unanswered. Its average is the CSAT score. A prebuilt **Customer satisfaction** dashboard uses it.

## Consequences

- Surveys depend on the pack being installed and its workflows published, like other service desk automation. A fresh MSP installs it with the module.
- Answers are tied to the survey, not the person who opened the link: whoever has the link can answer, once.
- NPS campaigns and client health (§8.9) build on the same module.
