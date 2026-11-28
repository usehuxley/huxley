# Workflows

A workflow is a document in the Huxley Workflow Format (HWF, ADR-004): what starts it, the steps it runs, and what it returns. Build it in the visual editor or as YAML (both edit the same draft), publish it, and Huxley runs it durably: every step's result is recorded before the next starts, so runs survive crashes and restarts (ADR-003, ADR-021). The JSON Schema is in [`schemas/hwf/v1.json`](../schemas/hwf/v1.json); expressions are described in [expressions.md](expressions.md).

```yaml
apiVersion: usehuxley.io/v1
kind: Workflow
metadata:
  key: onboard-user            # unique in the organization
  name: Onboard user
spec:
  inputs:                      # JSON Schema; runs are checked against it
    type: object
    required: [email]
    properties:
      email: { type: string }
  triggers:
    - type: webhook
      auth: { type: hmac, secret: "{{ secrets.hook_secret }}", prefix: "sha256=" }
  steps:
    - id: create
      type: http.request
      with:
        method: POST
        url: "https://api.example.com/users"
        headers: { Authorization: "Bearer {{ secrets.api_token }}" }
        body: { email: "{{ inputs.email }}" }
      retry: { max_attempts: 4, on: ["429", 5xx] }
    - id: tell
      type: core.notify
      with: { to: "role:admin", title: "Created {{ inputs.email }}" }
  outputs:
    id: "{{ steps.create.output.body.id }}"
```

## The visual editor

The **Visual** tab shows the workflow as a flow chart, from **Start** at the top:

- **Branches:** `if`, `switch`, `parallel`, `foreach` and `try` steps fan out into labelled branches that meet again below.
- **If the run fails:** these steps sit in their own column.
- **Adding steps:** select **+** on any connection, choose a step type, and it's inserted there.
- **Editing steps:** select a step to edit it in the panel beside the canvas.
- **Expressions:** expression and JSON fields are code editors. Inside `{{ }}` they complete what the step can read (its inputs, the steps that have run by then and their outputs, loop items, run variables, filters and functions). A problem in the field, such as a syntax error or a step that hasn't run yet, is underlined as you type, with the same message publishing would give.
- **Problems:** shown as a badge on the step they belong to.

Connections follow from the order and nesting of the steps, so the canvas can't build a workflow the format doesn't allow. Scroll or drag to move around, zoom with the controls, and use the minimap for large workflows. The arrows in the corner switch the flow between **top to bottom** and **left to right**; your browser remembers the choice. The **YAML** tab edits the same draft.

## Lifecycle

- **Draft:** saved at any time, even with problems; the editor lists what would stop it publishing, with where each problem is.
- **Publish:** makes the draft the next immutable version, after checking its structure, every expression (unknown names, references to steps that can't have run yet), and every `integration.action` step (the integration exists for the workflow's organization and has the action). Runs always pin to a version, and to the integration versions current when they start.
- **Test run:** runs the draft without publishing it (needs edit rights).
- **Archive:** stops triggers and runs; history stays. Restore brings the published version's triggers back.
- **Compare** any two versions, or a version and the draft.

## Folders

Folders organize the Workflows page. The folder tree sits beside the list, with how many workflows each folder holds.

- **Folders belong to an organization** and can be nested. A workflow is in one folder of its organization, or none (**Unfiled**).
- **Making, renaming, moving and deleting folders** needs `workflow:write` at the folder's organization. Deleting a folder moves what was in it up a level; it never deletes a workflow.
- **Moving workflows:** press **Move workflows**, choose them, and pick a folder. You can also use `POST /api/v1/workflow-folders/move` (`{ "workflowIds": [...], "folderId": ... }`).
- **Search and the status filter** look in every folder.
- **The System folder** holds the workflows installed from Huxley's built-in packs. It's made in an organization the first time one is installed there.
  - It can't be renamed, moved or deleted, and has no subfolders.
  - It holds only built-in workflows, and they stay in it.
  - They can still be edited, published and archived as usual.
- **Other packs:** a pack's new workflows go into a folder named after the pack. You can rename, move or delete it like any other.

The API is `/api/v1/workflow-folders`. Each workflow has a `folderId`, and `GET /api/v1/workflows?folderId=` lists one folder's workflows.

## Templates

**Workflows → start from a template** lists ready-made workflows:

- **Request and notify:** a form, and a workflow that tells your team about each request.
- **Form that runs a script:** a form whose answers become a library script's parameters.
- **Approve, then call an API:** a webhook, an approval, then an HTTP request.
- **Scheduled health check:** calls a URL on a schedule and alerts when it fails.

You fill in a few **bindings**, such as who to tell or which script, and get a **draft**, plus the template's form if it has one, to review and publish. Binding values are checked (recipients, URLs, script keys) and inserted as values, so they can't change the workflow's structure or become expressions. Using a template needs `workflow:write`, and `form:manage` when it includes a form. The API is `GET /api/v1/templates` and `POST /api/v1/templates/{key}/use`.

**Your own templates.** **Save as template** on a workflow's page saves its draft as a template of its organization. That needs `workflow:publish`. The template is offered there and in every organization below, next to the built-ins. If a form starts the workflow, the form is saved too, and each use of the template gets its own copy, with the workflow's trigger pointing at it. Delete one from the gallery. The API is `POST /api/v1/workflows/{id}/template`, `GET /api/v1/templates?orgId=…` and `DELETE /api/v1/templates/{id}`.

## Triggers

| Type | Starts a run when… |
|---|---|
| `manual` | Someone runs it from the app or the API. (Always possible, listed or not.) |
| `schedule` | A five-field `cron` (in `timezone`, else the organization's time zone, else UTC) or an `interval` like `15m` comes round. Missed times run once, then the schedule continues. |
| `webhook` | `POST /hooks/{id}` arrives (the URL is on the workflow page and stays the same across versions). Auth: `hmac` (signature header, SHA-256/512/1, optional prefix and signed timestamp), `bearer`, or `basic`. The body becomes the inputs. `response: sync` waits (up to `timeout`, at most 5m) and answers with the outputs; `async` answers 202 with the run id. With a signed timestamp (`timestamp_header`), requests more than 5 minutes old are refused, and a request already received in that window starts no run: it gets `200 {"duplicate": true}`. Without one, a captured request can be replayed, so use a timestamp when the sender supports it. |
| `event` | A domain event matches `event` (e.g. `org.created`, or `org.*`) in the workflow's organization or below, and the `filter` expression is true. The run happens in the event's organization. |
| `poll` | At each `interval` (at least `1m`), calls an integration's list action (`action`, optional `connection`, `with` passed as-is) and starts a run for each item it hasn't seen: `on: new` (default) or `new_or_changed`. `items` is where the list is in the response (e.g. `value`), `item_id` what identifies an item (default `id`). The first check only records what's there. The item is `trigger.item`, and `trigger.change` is `new` or `changed`. A failing check shows on the workflow's Triggers card. |
| `form` | Someone submits the form with this `form` key. The answers are the inputs, and the run happens in the organization it was submitted for, acting as the submitter. See [forms.md](forms.md). |
| `mcp_tool` | Offers the published workflow as a tool in the [MCP server](mcp.md#your-workflows-as-tools) and the assistant, named `run_<key>`, with its inputs as the tool's input schema. Optional `description` (default: the workflow's). |

## Steps

| Type | What it does |
|---|---|
| `http.request` | Calls an API. Output: `{status, headers, body}`. 4xx/5xx fail the step (with `status`, so retries can match) unless `allow_error_status`. Only public addresses are reachable unless the operator allows private ones; cloud metadata addresses never are. |
| `core.notify` | Notifies `user:<id>`, `team:<id>`, `role:<key>` (holders at the run's organization), in the app and by email, as each person's preferences for its `kind` say. `kind` is `workflow.notification` (the default), or a service desk kind: `ticket.assigned`, `ticket.replied` or `ticket.sla`. `exclude` lists user ids to leave out, such as whoever made the change. Sent exactly once. |
| `integration.action` | Calls an action of an imported integration: `action: <integration>/<action>`, inputs under `with` (parameters by name, the request body as `body`), and optionally `connection: <key>`. Output: the response body. See [integrations.md](integrations.md#calling-actions). |
| `subworkflow` | Runs another published workflow (found by key here or in an organization above) and, by default, waits for its outputs. |
| `core.contact.find` | Finds contacts by `email` and/or `external_id` (`{ entra: "<object id>" }`) among the run's organization and those below it. Output: `{ contacts, total }`. Needs the [Clients and contacts](crm.md) module. |
| `core.contact.upsert` | Updates the contact matched by `match` (`email`, the default, or an external id's system such as `entra`) at `org_id` (default: the run's organization), or adds one: `first_name`, `last_name`, `email`, `phone`, `mobile`, `job_title`, `external_ids` (merged). Fields left out keep their values. Output: `{ contact, created }`. |
| `core.company.get` | An organization's company details and sites (`org_id`, default the run's). Output: `{ company, sites }`. |
| `core.company.update` | Changes company details (`status`, `relationship`, `website`, `phone`, `notes`); the rest stays. Output: `{ company }`. |
| `core.ticket.embed` | Embeds tickets at the run's organization and below whose embedding is missing or out of date (their title or description changed, they moved organization, or the embeddings provider's model changed), newest changes first, up to `limit` (default 200), with each organization's embeddings provider. Does nothing without pgvector in the database. A provider that fails is reported without failing the step. Output: `{ available, embedded, errors: [{ org_id, error }] }`. Needs the [Service desk](servicedesk.md#search-by-meaning) module. |
| `core.timesheet.check` | Finds whom to remind about timesheets. `check` is `unsubmitted` (the last period that ended at least `days_late` days ago, default 1, still open or rejected, for people who log time or have a schedule), `under_logged` (a `date`, default yesterday, logged under the schedule, not on time off) or `missing_notes` (this period's entries without notes). Output: `{ check, people: [{ user_id, person, email, period_start, period_end, status, logged_minutes, expected_minutes, missing_notes, days_late, leads, recipient, lead_recipients }] }`; `recipient` and `lead_recipients` go straight into `core.notify`'s `to`. Needs the workforce module. |
| `core.calendar.sync` | Reads people's busy time from Microsoft 365 (free/busy, through the connection set under Settings → Calendars) and from iCalendar feeds, replacing their busy blocks from a day back to 60 days ahead. `user_id` syncs one person. Raises `calendar.conflict` once for each new clash with an upcoming appointment. Output: `{ people, blocks, failed: [{ user_id, error }], conflicts }`. Needs the workforce module (see [Calendars](workforce.md#availability)). |
| `core.calendar.write` | Puts an appointment (`appointment_id`) in its person's Microsoft 365 calendar when writing back is on: created when booked, moved with it, moved to the new person's calendar when it changes hands, removed when cancelled. Output: `{ written, action, event_id }` or `{ written: false, reason }`. |
| `core.csat.survey` | Asks a resolved ticket's requester how it went (the `cx` module; see [Satisfaction surveys](csat.md)). `with: { ticket }`. Output: `{sent, skipped, survey, emailed}`. |
| `core.nps.send` | Asks contacts of the run's organization and below how likely they are to recommend you (the `cx` module; see [net promoter surveys](csat.md#net-promoter-surveys)). `with: { campaign, limit }` (optional). Output: `{asked, emailed, campaign}`. |
| `core.health.compute` | Scores the client companies of the run's organization and below for today (the `cx` module; see [Client health](health.md)); declining ones raise `client.health_declining`, and falling sentiment `client.sentiment_declining`. Output: `{companies, declining}`. |
| `core.sentiment.record` | Stores a client message's sentiment, -2 to 2 with a reason (the `cx` module; see [Client health](health.md#sentiment)). Output: `{id, org_id, ticket_id, score}`. |
| `core.triage.options` | The queues (with descriptions) at a ticket's organization or above, and the priorities, for an AI step (see [AI triage](servicedesk.md#ai-triage)). Output: `{queues, queue_names, priority_keys, guide}`. |
| `core.triage.apply` | Records an AI triage answer (`queue`, `priority`, `reason`): with `ticket_id`, kept and applied while no one has changed the ticket; with `evaluation_id` and `sample_id`, recorded in an accuracy evaluation. Output: `{mode, applied, queue_id, priority}`. |
| `core.dispatch.recommend` | Who should take a ticket, best first, with reasons and cautions (see [Who should take it](servicedesk.md#who-should-take-it)). Output: `{recommendations, top}`. |
| `core.workforce.insights` | Skill gaps now: coverage of each queue's and ticket type's requirements, recent work done without them, and certifications expiring within `days` (default 60) (the `workforce` module; see [Skill gaps](workforce.md#skill-gaps)). Output: `{coverage, unqualified, expiring, summary}`. |
| `core.ticket.insight` | Shows a ticket's AI `summary`, its `entities` (`{people, devices, applications, errors}`) and a `next_step` on the ticket (see [Ticket insights](servicedesk.md#ticket-insights)). Output: the insight. |
| `core.kb.draft` | Drafts a knowledge article (`title`, Markdown `body`) tagged `ai-draft`, submits it for review unless `submit` is false, and links it to `ticket_id` (the `knowledge` module). Output: `{article_id, status}`. |
| `core.time.suggest` | Suggests `minutes` on a ticket to `user_id` (default its assignee) unless they logged time on it; they log or dismiss it from My time (see [Auto-documentation](servicedesk.md#auto-documentation)). Output: `{suggestion}`. |
| `core.report.send` | Emails a shared or prebuilt dashboard (`dashboard_id`), with the numbers of the run's organization and below over `range`, as `format` (`pdf`, `xlsx` or `csv`) to `to` (`user:`, `team:` or `role:` entries) (see [Download and email](analytics.md#download-and-email)). Output: `{sent, file_name}`. |
| `core.change.decide` | Records a submitted change's decision (`decision`: approved or rejected, `by`, `comment`), raising `change.approved` or `change.rejected` (see [Change management](changes.md)). Output: the change. |
| `core.project.scope.decide` | Records a submitted project scope change's decision (`scope_change_id`, `decision`: approved or rejected, `by`, `comment`). An approved change's hours are added to the budget; raises `project.scope_change_approved` or `project.scope_change_rejected` (see [Projects](projects.md)). Output: the scope change. |
| `core.quote.review` | Records a quote review's decision (`review_id` from `quote.submitted_for_review`, `decision`: approved, changes_requested or rejected, `by`, `comment`). Only the quote's open review counts; a changed quote fails it with `kind: conflict`. Raises `quote.approved`, `quote.changes_requested` or `quote.rejected` and tells the quote's owner (see [Sales](sales.md#internal-review)). Output: the review state. |
| `core.sales.log_email` | Logs an email from a sales mailbox (`email_id`) as activities on each contact it's from or to (on their client's latest open opportunity) and on open leads with those addresses; each once. Output: `{logged, unmatched: [address]}` (see [Sales](sales.md#logging-email)). |
| `core.scope.context` | Gathers what drafting a scope reads (`scope_id`): the opportunity, its discovery numbered [1], [2]…, the client's environment and agreements, and past approved scopes of the same kind. Output: `{text, sections, sources}` (see [Sales](sales.md#scoping)). |
| `core.scope.write` | Keeps a drafted Scope of Work (`scope_id`, `sections: [{key, body, sources}]`, `questions: [{section, question}]`), leaving sections people edited alone; or, with `error`, records that drafting failed. Output: `{written}`. |
| `core.scope.estimate_context` | Gathers what estimating a scope reads (`scope_id`): its Scope of Work, the client's environment, project templates and the hours logged on past projects' tasks, plus numbered labour roles and catalog products to pick from. Output: `{text, roles, products}`. |
| `core.scope.write_estimate` | Keeps a drafted estimate (`scope_id`, `tasks: [{phase, name, role, low, likely, high, milestone, deliverable}]`, `items: [{product, name, quantity, basis, alternate}]`, `risk`), keeping rows people edited; or, with `error`, records that estimating failed. Output: the estimate. |
| `core.project.status` | What happened on one project (`project_id`) or every active project in the run's organization and below, over the last `days` (default 7). Output: `{ projects: [{ project_id, name, client, status, period_start, period_end, health, health_reasons, budget_hours, used_hours, burn_percent, hours_in_period, tasks_done, tasks_total, completed, waiting, overdue, milestones_reached, milestones_upcoming, scope_changes_pending, manager_recipient }] }`; `manager_recipient` goes straight into `core.notify`'s `to` (see [Projects](projects.md#status-reports)). |
| `core.project.report` | Saves a status report on a project: `project_id`, `body` (Markdown), `client_visible` (default false), `period_start`, `period_end`, `author` (default "AI status reports"). Raises `project.status_report`. Output: the report. |
| `core.asset.sync` | Runs dataset → asset mapping rules: give `mapping_id` for one rule, or `dataset` (a dataset's key) for every rule on it that's on. The dataset's rows become assets, matched, updated and created by each rule, in one transaction per rule. Put it after the step that fills the dataset. Output: `{ rows, created, updated, unchanged, skipped, problems: [{ row, reason }] }`, summed over the rules, plus `mappings` (how many ran) given `dataset`. Needs the [Assets](assets.md#syncing-from-datasets) module. |
| `core.asset.set_warranty` | Records a warranty a lookup found for an asset at the run's organization or below: `asset_id`, `source` (the lookup's name, e.g. `dell`), `ends_on` (`YYYY-MM-DD`), and optionally `starts_on`, `kind` (`manufacturer`, `extended`, `support`, `other`), `provider`, `reference`, `notes`. A warranty the same lookup recorded with the same reference is updated. Output: the warranty. Needs the [Assets](assets.md#warranties) module. |
| `core.kb.embed` | Embeds published knowledge base articles at the run's organization and below whose embedding is missing or out of date (their published title or text changed, they moved organization, or the provider's model changed), up to `limit` (default 200), with each organization's embeddings provider. Does nothing without pgvector. Output: `{ available, embedded, errors: [{ org_id, error }] }`. Needs the [Knowledge base](knowledge.md#search-by-meaning) module. |
| `core.mail.fetch` | Fetches new mail from the enabled IMAP and Microsoft 365 mailboxes at the run's organization and below (or just `mailbox_id`), up to `limit` per mailbox (default 50). Each new message is kept and raises `email.received`. A mailbox that can't be reached is reported, and its error kept on it, without failing the step. Output: `{ mailboxes: [{ mailbox_id, address, fetched, new } or { mailbox_id, address, error }] }`. Needs the [Service desk](servicedesk.md#email) module. |
| `core.ticket.email_reply` | Emails a public note (`note_id`, from `ticket.note_added`) to its ticket's requester, from the mailbox the ticket's mail came to (else the nearest at its organization or above). The subject has the ticket's `[HX#…]` token, and the message continues the email thread. Internal notes, notes from the requester, and notes already sent are skipped. Output: `{ sent: true, to, message_id }` or `{ sent: false, reason }`. Needs email set up for the instance. |
| `core.ticket.create` | Opens a ticket at `org_id` (default: the run's organization) with `title` and optionally `description`, `type`, `impact`, `urgency`, `priority`, `queue_id`, `requester_id`, `assignee_user_id`, `assignee_team_id` and `source` (default `workflow`). The SLA applies as usual. Output: `{ ticket }`. Needs the [Service desk](servicedesk.md) module, as do the other ticket steps. |
| `core.ticket.get` | Reads a ticket (`ticket_id`), with its notes if `notes` is true. Output: `{ ticket, notes }`. |
| `core.ticket.find` | Finds tickets at the run's organization and below (or `org_id`'s): by `queue_id`, `assignee` (a person's id, or `none`), `state` (`open`, the default, or `resolved`, `closed` or `all`), `search` and `priority` (e.g. `p1,p2`), most urgent first, at most `limit` (default 50). Output: `{ tickets, total }`. |
| `core.ticket.update` | Changes a ticket as it is now: `title`, `description`, `impact`, `urgency`, `priority` (or `auto`), `queue_id`, `requester_id`, `assignee_user_id` and `assignee_team_id` (`null` clears), then `status`, with `resolution` where the status needs one. Output: `{ ticket }`. |
| `core.ticket.note` | Adds a note (`body`) of `kind` `internal` (the default), `public` or `resolution`. Notes from workflows aren't emailed to the requester. Output: `{ note }`. |
| `core.ticket.dispatch` | Assigns a ticket (`ticket_id`) by its queue's [dispatch](servicedesk.md#dispatch) setting, or by `strategy` (`round_robin`, `load` or `skills`). A ticket that's already assigned is left alone unless `reassign` is true. Output: `{ assigned, assignee, strategy, reason, considered }`. `reason` is `dispatched`, or why nobody was chosen: `assigned`, `no_queue`, `manual` or `nobody_available`. |
| `core.ticket.from_email` | Turns a received email (`email_id`, from `email.received`) into ticket work, following its mailbox's rules. It skips automatic mail and loops, adds a reply to the ticket it belongs to (reopening a resolved one), or opens a ticket for the sender, with the email's attachments on it. Output: `{ outcome, detail, ticket_id, created, attachments }`. Running it again for the same email changes nothing. Needs the [Service desk](servicedesk.md#email) module. |
| `transform` | Computes a value. |
| `set_var` | Sets run variables, read as `run.vars.<name>`. |
| `approval` | Pauses until one of the `approvers` (`user:<id>`, `team:<id>`, `role:<key>` at the run's organization, or client contacts in the portal: `contact:<id>`, `client_approvers`) approves or rejects under **Approvals** (contacts: in the [portal](portal.md#approvals)). They're notified with a link. The first decision counts. With `changes: true` they may also **request changes**. Output: `{decision: approved \| rejected \| changes_requested, by: {id, name}, comment, decided_at}`. `expires` (default `7d`) fails it with `kind: timeout`. |
| `core.email` | Emails any address through the instance's mail server (`email.smtp_url`). `with`: `to` (an address or a list, at most 20; each gets their own copy), `subject`, `text`, and optionally `html` (otherwise the text, escaped). Output: `{sent, to}`. Fails with `kind: email` if mail isn't set up. For Microsoft Teams, post to a channel's **Workflows** webhook URL with `http.request`; for Slack, use the built-in Slack integration. |
| `dataset.query` | Reads rows of a [dataset](datasets.md): the run's organization's dataset with key `with.dataset`, or the nearest above it. Only rows of the run's organization and those below it are read. `with`: `filters` (column values rows must equal), `org_id` (one organization's rows only), `order_by` (a column; `-column` descending), `limit` (default 100, at most 1000). Output: `{rows, total}`. |
| `dataset.upsert` | Writes rows to a dataset. `with`: `dataset`, `rows` (a list of objects of column values), `org_id` (the organization they belong to: the run's, which is the default, or one below it), `replace` (`true` replaces all of that organization's rows), `map` (build each row from an item of `rows`: column → dot path into the item, e.g. `{ upn: userPrincipalName, city: address.city }`). Rows whose key exists are updated. Rows can't contain secrets or anything derived from them. Output: `{written}`. |
| `ai.prompt`, `ai.classify`, `ai.extract` | Ask the organization's [AI provider](ai.md): a prompt answered with `{text, model}`; one of `labels` for some `text` (`{label, model}`); or data from `text` as a JSON `schema` describes it. `model` overrides the provider's model, and `system` or `instructions` guide it. They refuse input carrying secrets. Failures have `kind: ai`. |
| `wait_for_callback` | Pauses until another system calls a single-use URL, then continues with the request (`method`, `query`, `content_type`, `body`) as output. `send` tells the other system the URL (`{{ callback.url }}`) with an integration action (`action`, `with`) or an HTTP request (`request`). `expires` (default `24h`) is how long it waits before failing with `kind: timeout`. Needs `auth.public_url`. |
| `script` | Runs the library script `script` (this organization's, or the nearest one above with that key) with the parameters in `with`, which are checked against the script's parameter schema. A runner picks it up and runs it in a sandbox. Output: what the script returns, which is PowerShell's pipeline output or Python's `result` variable. `timeout` (default `15m`) includes waiting for a runner. A failed script fails the step with `kind: script`, its `exit_code`, and the end of its `logs`. `env` sets environment variables, which can come from variables or secrets. `connections` (`NAME: <integration>` or `<integration>/<connection>`) gives the script a connection's credentials, fetched when it starts: `NAME_URL`, and `NAME_TOKEN` (OAuth 2.0 or bearer), `NAME_KEY` (API key), or `NAME_USERNAME` and `NAME_PASSWORD` (basic). Secret values and credentials are never put in the parameters, and are scrubbed from the output and logs. See [runners](runners.md). |
| `wait` | Pauses for a `duration` or `until` a time. |
| `if`, `switch` | Run one block depending on a condition or a value. |
| `foreach` | Runs its steps for each item (`as` names the item; `concurrency` runs several at once). Output: the last step's output for each item. |
| `parallel` | Runs branches at once; `join: any` finishes with the first to succeed. |
| `try` | Runs `catch` if its steps fail (with `error`), and `finally` either way. |
| `return`, `fail` | End the run with outputs, or with an error. |

Every step has an `id` (unique in the workflow; others read its result as `steps.<id>.output`) and can have `if` (skip unless true), `timeout`, `retry` (`max_attempts`, `backoff: fixed | exponential | jitter`, `delay`, `max_delay`, `on`: statuses like `429`, `4xx`, `5xx`, `timeout`, `network`), and `continue_on_error`. `spec.on_error` runs when nothing caught a failure. `spec.settings` takes a run `timeout` and a `concurrency` limit (runs beyond it wait their turn).

## Variables and secrets

Organization variables (on the organization's page) are inherited down the tree; a nearer organization can set its own, and an organization can lock a variable for those below it. Workflows read them as `vars.<key>`. Secret variables are encrypted, never shown again, and read only as `secrets.<key>`: any step input that used one is stored redacted (the real value is encrypted for the step to use), and secret values are masked in what such steps return.

**Secret inputs.** An input marked `writeOnly: true` in the workflow's input schema, such as a password or a PIN, is kept encrypted with the run. The run's inputs show `«redacted»`. Steps still read it as `inputs.<name>`, and reading it works like reading `secrets.*`: whatever used it is stored redacted, and its value is masked in what those steps return.

## Runs

The run page shows each step as it starts and finishes, with its input, output or error and attempts. Inputs and outputs over 256 KB (configurable) are kept in object storage: the page shows their size and a download link, and `GET /api/v1/runs/{id}/steps/{key}/{attempt}/{input|output}` returns them; expressions see the full value as usual. A failed run can be **resumed** after fixing the cause: steps that succeeded aren't run again. Running runs can be **cancelled**: nothing new starts, and subworkflows they started stop too. All runs are under **Runs**.

## Running from a step

On a run's page, **Run again from here** on a top-level step starts a **test run** of the current draft at that step. Every step before it keeps this run's results instead of running again, including loop iterations and secrets, which are re-encrypted for the new run. It also reuses this run's inputs. That's handy when iterating on a later step without re-running slow or side-effecting earlier ones. The API is `from: { runId, stepId }` on `POST /api/v1/workflows/{id}/runs` with `test: true`. Give `inputs` to change them.

## Identity

A run started by hand acts as whoever started it. Runs started by a schedule, webhook or event act as the workflow's **run-as** service account (set on the workflow page), or as no one if none is set; subworkflows act as their parent. The identity is `actor.id` in expressions and is recorded on the run. Setting it needs `workflow:publish` and `service_account:manage`, since whoever hands out an account's identity could act as it. If the account is disabled, triggers start nothing (a webhook answers 409) until it's re-enabled or changed.

## Scripts

**Scripts** is a library of PowerShell 7 and Python 3 scripts. Each save is a new, numbered version, and earlier versions stay readable. Every script has a JSON Schema of its parameters, which a step running it will ask for. A library at an organization is shared with the organizations below it. Scripts are edited in a code editor: highlighting for their language, completion of their parameters as the script receives them (`$Name` in PowerShell, `inputs["name"]` in Python), and the parameters schema checked as you type.

The `script` step runs them on [runners](runners.md), each script in a fresh rootless Podman container (design doc §6.9). A PowerShell script gets its parameters as `param(...)` arguments. A Python script gets them as the `inputs` dict and sets `result`. A JavaScript script (run with Node.js, as an ES module) can `export default` its result, or an `async` function of the inputs (`export default async ({ name }) => ({ greeting: `Hi ${name}` })`), or set `globalThis.result`. `fetch` works through the egress policy. Viewing scripts needs `script:read`, and changing them needs `script:manage`.

## Permissions

| Permission | Allows |
|---|---|
| `workflow:read` | Viewing workflows, runs and variables |
| `workflow:write` | Creating and editing drafts, test runs |
| `workflow:publish` | Publishing, archiving, restoring |
| `workflow:run` | Starting, cancelling and resuming runs of published versions |
| `variable:manage` | Setting variables and secrets |

All are granted per organization and inherited by the organizations below. Technicians have read and run; read-only users have read.

## Workflows as code

`huxley workflows` keeps workflows in files (HWF YAML or JSON), for example in a Git repository, and syncs them with a Huxley server (design doc §6.7). To write them in TypeScript, see the [TypeScript SDK](sdk.md).

```sh
export HUXLEY_SERVER=https://huxley.example.com
export HUXLEY_TOKEN=hxp_…          # an API token: Profile → API tokens
export HUXLEY_ORG="Contoso MSP"    # the organization's name or id

huxley workflows pull --dir workflows/           # write every workflow's draft as YAML
huxley workflows validate workflows/             # check files locally (no server needed)
huxley workflows diff workflows/                 # how the files differ from the server's drafts
huxley workflows push workflows/                 # save them as drafts (creating new workflows)
huxley workflows push workflows/ --publish --notes "Release 12"
huxley workflows run greet -i name=Ada           # run the published version, print its outputs
huxley workflows run greet --draft -i name=Ada   # test-run the draft
huxley workflows test workflows/                 # run the *.test.yaml fixture tests
```

Things to know:

- **Exit codes:** `validate` and `diff` exit 1 when there are problems or differences, so they work as CI checks. `run` exits 1 if the run fails.
- **Pulled files** use the same key order as the YAML tab in the editor (other keys sorted), so they diff cleanly. They start with a `yaml-language-server` comment pointing at the HWF schema, so editors autocomplete them.
- **Matching:** files are matched to workflows by `metadata.key`. The same key in two files is an error.
- **Folders are directories.**
  - `pull` writes each workflow into its [folder](#folders)'s directory (`workflows/System/…`, `workflows/Ops/Daily/…`), and unfiled ones at the top. When a workflow has moved, its old file is removed.
  - Given a directory, `push` files each workflow by where its file is, making folders as needed (top level: unfiled), and `diff` reports folders that differ. A file named on its own keeps its workflow where it is.
  - Built-in workflows stay in the System folder whatever their file's directory: `push` says so and carries on.
  - `validate`, `diff` and `push` look in subdirectories, but not hidden ones such as `.git`.
- **Permissions:** `push` needs `workflow:write`, and `--publish` also needs `workflow:publish`. Use a token scoped to what CI needs.
- **Inputs:** `-i name=value` parses the value as JSON when it can (`-i count=3`, `-i 'tags=["a"]'`), and `--inputs-file` takes a JSON object.

### Fixture tests

A `*.test.yaml` file next to a workflow file describes test cases:

```yaml
workflow: lookup.yaml          # relative to this file
cases:
  - name: uses the API's count
    inputs: { name: Ada }
    mocks:                     # these steps don't run: they return this
      fetch: { status: 200, body: { count: 3 } }
    expect:
      outputs: { line: Ada has 3 friends }
  - name: fails when the API can't be reached
    inputs: { name: Ada }
    expect: { status: failed, error: { kind: network } }
```

How tests run:

- **On the server, without saving anything.** `huxley workflows test` test-runs each case's copy of the workflow file there, so integrations, connections and variables behave as they do for real.
- **Mocks:** a mocked step doesn't run. It returns the given output, which later steps read as usual. Mock a step inside a loop with its iteration indices, e.g. `inner[2][0]`.
- **Expectations:**
  - `expect.status` defaults to `succeeded`.
  - `outputs` and `error` match as subsets: every key you give must be there, equal, and extra keys are fine.
- **Exit code:** the command exits 1 if any case fails.
- **The API:** `POST /api/v1/workflows/test-runs` with `{ orgId, document, inputs, mocks }`. It needs `workflow:write`.

Folders passed to `validate`, `diff` and `push` skip `*.test.yaml` files.

### Publishing from Git

Keep workflows in a repository and let CI publish them. Pull requests validate, test and show the diff, and merges to the main branch publish. Ready-made pipelines are in [`examples/git-sync`](../examples/git-sync):

- [GitHub Actions](../examples/git-sync/github-actions.yml)
- [GitLab CI](../examples/git-sync/gitlab-ci.yml)
- [Azure Pipelines](../examples/git-sync/azure-pipelines.yml)

Each needs `HUXLEY_SERVER`, `HUXLEY_ORG` and a secret `HUXLEY_TOKEN`. The token should belong to a service account and be scoped to `workflow:read`, `workflow:write` and `workflow:publish`. The pipelines run the CLI from the `ghcr.io/usehuxley/huxley` image, so pin the version that matches your server. Each published version's notes record the commit.

To start a repository from what's already in Huxley, run `huxley workflows pull --dir workflows/` and commit the result.
