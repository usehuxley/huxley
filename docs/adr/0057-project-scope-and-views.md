# ADR-057: Scope changes are client-approved through a workflow; template workflows are filtered events

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

ADR-056 covered projects, templates, task tickets and budgets. Phase 9 also asks for scope change requests approved by the client, list, board, timeline and calendar views, projects linked to agreements, and templates with attached workflows.

## Decision

- **Scope changes** (`prj_scope_changes`) are numbered per project, with a title, a reason, hours (positive or negative) and an optional price. Their states are draft, submitted, approved, rejected and withdrawn.
  - Submitting raises `project.scope_change_submitted`.
  - The decision is recorded by the step `core.project.scope.decide`, the same way changes are decided (ADR-054). Approval is a workflow, so who approves (the client, the provider first, or both) can be changed.
  - The `huxley/project-scope-approvals` pack asks the client's approvers in the portal.
  - An approved change's hours are added to the project's budget, and the budget thresholds are rechecked. It raises `project.scope_change_approved` or `_rejected`.
  - Only drafts can be changed.
- **The portal** lists scope changes once they've been submitted, with their state.
- **Task dates:** tasks have an optional start and due date. Templates can date them in days, relative to the project's start, as they already do for milestones.
- **Views:** a project's tasks show as a list (by phase), a board (columns by status category, so custom statuses fall into place) and a timeline (task bars and milestones). The calendar view comes with the dispatch calendar's capacity work.
- **Agreements:** a project can bill against one of its client's agreements. Phase 11 billing uses the link and the billing method.
- **Template workflows are workflows on project events, filtered to the template.** Every project event carries `projectId` and `templateId`. A template's page lists the workflows whose project-event trigger filter names its id, and can draft one (`project.created`, filtered to the template) to start from. Nothing new dispatches workflows: event triggers and filters already do.

## Consequences

- Scope approval follows the same pattern as change approval: a pack plus a decide step.
- Linking a workflow to a template is just a filter on its event trigger, so a workflow can serve several templates, or all of them.
