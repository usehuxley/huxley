# ADR-059: Status reports are drafted by a pack and shared by a person

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 9 asks for scheduled, AI-drafted project status reports. Huxley already has AI steps that use the customer's own provider (`ai.extract` with structured output), scheduled workflows, and a portal that shows a client their projects.

## Decision

- **Reports** (`prj_status_reports`) are Markdown with an optional period. Each records the project's health at the time, who wrote it, and whether the client sees it. Saving one raises `project.status_report`.
- **Two steps** split the work:
  - `core.project.status` gathers the facts for one project, or every active project in the run's organization and below, over the last few days: what got done, hours, budget, milestones, overdue and waiting tasks, pending scope changes, health and the manager.
  - `core.project.report` saves a report.
  - Writing it up is an ordinary `ai.extract` step between them, so the prompt, the model and the format can be changed like any workflow.
- **The `huxley/project-status-reports` pack** runs every Monday at 07:00:
  - drafts a report for each active project (a summary, highlights, risks and next steps)
  - saves it, not shared
  - tells the project's manager
- **People stay in charge of what the client sees.** A drafted report is shared with the client only when someone with `project:manage` shares it. Managers can also write reports themselves. The portal shows the latest shared report on each project.
- Report bodies use a small Markdown subset (paragraphs, bold headings, bullets) rendered as text by `ReportBody` in `@usehuxley/ui`, so nothing in them is ever HTML.

## Consequences

- The AI only drafts. Nothing reaches the client without a person deciding.
- Other schedules, or reports emailed instead, are changes to the pack's workflow, not to Huxley.
