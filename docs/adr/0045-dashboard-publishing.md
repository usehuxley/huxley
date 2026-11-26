# ADR-045: Dashboards are published by approving a snapshot for an audience

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 8 asks for a publishing workflow for reports: a user builds a dashboard, submits it, and after approval it appears for a team and in the client portal. Until now only people with `dashboard:manage` could share a dashboard, and it was shared with everyone who sees dashboards at its organization.

## Decision

- **Audience on shared dashboards:** teams and roles (`audience_teams`, `audience_roles`). Both empty means everyone with `dashboard:read` there, as before. People who manage dashboards, and the dashboard's author, always see it. The client portal stays a separate flag.
- **Submissions** (`dashboard_submissions`): the author submits their personal dashboard with the audience asked for and a note. The submission keeps a **snapshot** (name, description, range, widgets). Reviewers approve what they saw, so edits after submitting don't slip in. There is at most one pending submission per dashboard.
- **Deciding:** `dashboard:manage` at the dashboard's organization, by someone other than the submitter.
  - Approving makes the dashboard shared, as the snapshot, with the audience. The submission gets the next publication number, so the history is the list of versions.
  - Rejecting needs a comment, and the dashboard stays personal.
  - The author can withdraw a pending submission.
- **Events, not hard-wired notifications:** `dashboard.submitted`, `dashboard.published` and `dashboard.rejected` go through the outbox. The built-in Dashboard publishing pack notifies administrators and authors, so who hears about it is a workflow anyone can change.
- **Copy:** `POST /dashboards/{id}/copy` makes a personal copy of anything you can see, so a published dashboard is revised by copying, changing and submitting again.

## Consequences

- Publishing doesn't need `dashboard:manage`, only an approver who has it.
- Audience narrows who sees a dashboard. It doesn't widen access: widgets still show only the numbers each viewer can read.
- Revising a published dashboard in place remains a `dashboard:manage` edit. A revision-submission flow (replacing a published dashboard on approval) can be added on the same table later.
