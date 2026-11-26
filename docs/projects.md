# Projects

Turn on **Projects** (Settings → Modules). It needs CRM and the service desk. Projects then appear in the sidebar for anyone with `project:read`. Technicians can work tasks and milestones, and creating projects and templates takes `project:manage` (ADR-056).

## Templates

**Settings → Project templates** holds the shapes projects start from:

- **Phases**, each with an optional budget in hours.
- **Tasks** in each phase: an estimate in hours, the day it **starts on** (counted from the project's start) and how many **days it takes**, a task it comes **after** (it shows *Waiting on another task* until that one is done), and whether **the client sees it** in the portal.
- **Milestones**, due a number of **days after the start**, and whether the client sees them.

**Workflows** on a template's page lists the workflows that run for its projects: those on a project event whose trigger filter names the template, e.g. `{{ event.payload.templateId == '<id>' }}`. **New workflow for this template** drafts one on `project.created`, ready to fill in (send a kickoff email, create a Teams channel, order hardware).

## Starting a project

**Projects → New project**: the client, a name, the start (and optionally the end), the billing method (time and materials, fixed fee, fixed fee by milestone, not to exceed, or covered by an agreement), the client's **agreement** it bills against, and a template. Choosing a template lists its tasks so you can pick **who does what**. The budget defaults to the template's phase budgets added up.

Creating it lays everything out. Phases and milestones are dated from the start, and every task becomes a ticket of the **Project task** type, assigned as you chose. Assignees see their tasks in **My Work** and log time on them like any ticket. You can add more tasks on the project's page.

## Views

A project's tasks show as:

- **List:** by phase, with hours used against estimates. **Edit** on a task changes its phase, estimate, dates and client visibility.
- **Board:** columns for to do, in progress, blocked and done.
- **Timeline:** each dated task as a bar from its start to its due date, milestones as diamonds, and today's line.

## Budget and health

The project's page shows:

- **Budget:** hours logged on its tasks against its budget, by phase too.
- **Progress:** tasks done of all of them.
- **Health:** **off track** with an overdue milestone or over budget, and **at risk** when the budget is being used more than 25 points faster than tasks are being finished. The reasons are listed.

Each budget threshold (50, 75, 90 and 100% by default) raises `project.budget_threshold` once, when time logged on a task first reaches it. If the burn drops back under a threshold (time deleted, or a bigger budget), that threshold can fire again.

## Capacity and who does what

**Projects → Capacity** shows, for each person and week:

- the hours they work: their schedule, less approved time off, or 40 hours a week without a schedule
- what's booked: appointments, plus the remaining estimates of their open project tasks spread over each task's dates
- weeks booked beyond capacity, flagged

Tasks without dates are counted as **undated**. Filter by team, and page through the weeks.

Template tasks can **need a skill** at a level, which becomes the task's requirement. For each unassigned task, the project page suggests up to three people who are qualified and available, with their skill match and free hours this week. Assign one with a click.

## Scope changes

When the work grows (or shrinks), draft a **scope change** on the project: what changes, the hours it adds (negative to remove) and optionally its price. **Submit** sends it for approval, and you can **withdraw** it until it's decided.

Install the **Project scope approvals** pack at the client (or above) to send submitted changes to the client's approvers in the portal. When they approve, the hours are added to the project's budget, and its thresholds are checked again. Change the pack's workflow to add a provider approval first, or to approve small changes automatically.

## Events

| Event | When | Payload |
|---|---|---|
| `project.created` | A project is created | `name`, `templateId` |
| `project.status_changed` | Its status changes | `name`, `from`, `to` |
| `project.milestone_reached` | A milestone is marked reached | `milestone`, `milestoneId` |
| `project.task_completed` | A task is resolved | `taskId`, `title` |
| `project.budget_threshold` | A threshold is reached | `name`, `threshold`, `percent`, `usedHours`, `budgetHours` |
| `project.scope_change_submitted` | A scope change is submitted | `scopeChangeId`, `projectName`, `number`, `title`, `description`, `hours`, `amount`, `currency` |
| `project.scope_change_approved`, `project.scope_change_rejected` | It's decided | `scopeChangeId`, `projectName`, `number`, `title`, `hours`, `amount`, `by`, `comment` |

Every project event also carries `projectId` and `templateId`.

The event's subject is the project, at the client company. Workflows decide what to do, for example email the account manager at 90%, or tell the client when a milestone is reached.

## In the portal

Client approvers and admins see **Projects** in the portal. It lists the company's projects marked for the portal (on by default, apart from cancelled ones), with their progress, the milestones marked for the client (reached or upcoming), the tasks marked for the client, and scope changes sent to them (waiting, approved or rejected). Hours used and the budget never show there.
