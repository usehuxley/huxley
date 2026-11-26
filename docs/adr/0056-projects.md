# ADR-056: Project tasks are tickets; budgets burn from time on them

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 9's project management covers projects with phases, tasks, milestones and templates, assigning tasks across technicians, budget tracking with alerts, and showing progress to the client. Huxley already has tickets (assignment, My Work, time, SLAs, notes) and time entries.

## Decision

- **A project task is a ticket** of a new `project_task` type (To do, In progress, Blocked, Done, Closed), with `source = project`. `prj_tasks` holds its project, phase, estimate, position, dependencies and client visibility. Technicians therefore see their tasks in My Work, log time on them, and work them like any ticket.
- **Projects** (`prj_projects`) belong to a client company and have phases (each with an optional budget), milestones (due date, reached, client visible), a status, a billing method (fixed fee, by milestone, time and materials, not to exceed, or covered by an agreement), budget hours and budget thresholds (default 50, 75, 90 and 100%).
- **Templates** (`prj_templates`) hold a structure: phases with tasks (key, estimate, dependencies, client visibility) and milestones as days after the start. The structure is checked: keys are unique, dependencies exist, and there are no cycles. Creating a project from a template lays out phases, milestones and task tickets, assigning tasks by key. The budget defaults to the phases' budgets added up.
- **Budget burn** is the time logged on a project's tasks against its budget. After any time entry on a task is logged, changed or deleted, each threshold newly reached raises **`project.budget_threshold`** once. A threshold the burn drops back under (from deleted time or a bigger budget) can fire again.
- **Events:** `project.created`, `project.status_changed`, `project.milestone_reached`, `project.task_completed` (a task resolved) and `project.budget_threshold`. What happens next (telling the manager, the client, or the account manager) is up to workflows.
- **Health** is computed rather than stored. A project is off track with an overdue milestone or over budget, and at risk when budget burn runs more than 25 points ahead of tasks done.
- **The portal** shows a company's projects (when marked for the portal) to client approvers and admins: progress, client-visible milestones, and client-visible tasks. It never shows hours or budget.
- **Permissions:** `project:read`, `project:write` (work tasks and milestones; technicians) and `project:manage` (create and change projects and templates). The `projects` module requires CRM and the service desk.

## Consequences

- No second task system: the ticket machinery covers projects unchanged.
- Scope changes, capacity planning, timelines and AI status reports build on these tables in later work.
