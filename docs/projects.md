# Projects

Turn on **Projects** (Settings → Modules). It needs CRM and the service desk. Projects then appear in the sidebar for anyone with `project:read`. Technicians can work tasks and milestones, and creating projects and templates takes `project:manage` (ADR-056).

## Templates

**Settings → Project templates** holds the shapes projects start from:

- **Phases**, each with an optional budget in hours.
- **Tasks** in each phase: an estimate in hours, a task it comes **after** (it shows *Waiting on another task* until that one is done), and whether **the client sees it** in the portal.
- **Milestones**, due a number of **days after the start**, and whether the client sees them.

## Starting a project

**Projects → New project**: the client, a name, the start (and optionally the end), the billing method (time and materials, fixed fee, fixed fee by milestone, not to exceed, or covered by an agreement) and a template. Choosing a template lists its tasks so you can pick **who does what**. The budget defaults to the template's phase budgets added up.

Creating it lays everything out. Phases and milestones are dated from the start, and every task becomes a ticket of the **Project task** type, assigned as you chose. Assignees see their tasks in **My Work** and log time on them like any ticket. You can add more tasks on the project's page.

## Budget and health

The project's page shows:

- **Budget:** hours logged on its tasks against its budget, by phase too.
- **Progress:** tasks done of all of them.
- **Health:** **off track** with an overdue milestone or over budget, and **at risk** when the budget is being used more than 25 points faster than tasks are being finished. The reasons are listed.

Each budget threshold (50, 75, 90 and 100% by default) raises `project.budget_threshold` once, when time logged on a task first reaches it. If the burn drops back under a threshold (time deleted, or a bigger budget), that threshold can fire again.

## Events

| Event | When | Payload |
|---|---|---|
| `project.created` | A project is created | `name`, `templateId` |
| `project.status_changed` | Its status changes | `name`, `from`, `to` |
| `project.milestone_reached` | A milestone is marked reached | `milestone`, `milestoneId` |
| `project.task_completed` | A task is resolved | `taskId`, `title` |
| `project.budget_threshold` | A threshold is reached | `name`, `threshold`, `percent`, `usedHours`, `budgetHours` |

The event's subject is the project, at the client company. Workflows decide what to do, for example email the account manager at 90%, or tell the client when a milestone is reached.

## In the portal

Client approvers and admins see **Projects** in the portal. It lists the company's projects marked for the portal (on by default, apart from cancelled ones), with their progress, the milestones marked for the client (reached or upcoming), and the tasks marked for the client. Hours and budget never show there.
