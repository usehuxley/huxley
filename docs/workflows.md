# Workflows

A workflow is a document in the Huxley Workflow Format (HWF, ADR-004): what starts it, the steps it runs, and what it returns. Build it in the visual editor or as YAML (both edit the same draft), publish it, and Huxley runs it durably: every step's result is recorded before the next starts, so runs survive crashes and restarts (ADR-003, ADR-021). The JSON Schema is in [`schemas/hwf/v1.json`](../schemas/hwf/v1.json); expressions are described in [expressions.md](expressions.md).

```yaml
apiVersion: usehuxley.com/v1
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

## Lifecycle

- **Draft:** saved at any time, even with problems; the editor lists what would stop it publishing, with where each problem is.
- **Publish:** makes the draft the next immutable version, after checking its structure, every expression (unknown names, references to steps that can't have run yet), and every `integration.action` step (the integration exists for the workflow's organization and has the action). Runs always pin to a version, and to the integration versions current when they start.
- **Test run:** runs the draft without publishing it (needs edit rights).
- **Archive:** stops triggers and runs; history stays. Restore brings the published version's triggers back.
- **Compare** any two versions, or a version and the draft.

## Triggers

| Type | Starts a run when… |
|---|---|
| `manual` | Someone runs it from the app or the API. (Always possible, listed or not.) |
| `schedule` | A five-field `cron` (in `timezone`, else the organization's time zone, else UTC) or an `interval` like `15m` comes round. Missed times run once, then the schedule continues. |
| `webhook` | `POST /hooks/{id}` arrives (the URL is on the workflow page and stays the same across versions). Auth: `hmac` (signature header, SHA-256/512/1, optional prefix and signed timestamp), `bearer`, or `basic`. The body becomes the inputs. `response: sync` waits (up to `timeout`, at most 5m) and answers with the outputs; `async` answers 202 with the run id. |
| `event` | A domain event matches `event` (e.g. `org.created`, or `org.*`) in the workflow's organization or below, and the `filter` expression is true. The run happens in the event's organization. |
| `poll` | At each `interval` (at least `1m`), calls an integration's list action (`action`, optional `connection`, `with` passed as-is) and starts a run for each item it hasn't seen: `on: new` (default) or `new_or_changed`. `items` is where the list is in the response (e.g. `value`), `item_id` what identifies an item (default `id`). The first check only records what's there. The item is `trigger.item`, and `trigger.change` is `new` or `changed`. A failing check shows on the workflow's Triggers card. |
| `form` | Someone submits the form with this `form` key. The answers are the inputs, and the run happens in the organization it was submitted for, acting as the submitter. See [forms.md](forms.md). |
| `mcp_tool` | Reserved for MCP (Phase 4). |

## Steps

| Type | What it does |
|---|---|
| `http.request` | Calls an API. Output: `{status, headers, body}`. 4xx/5xx fail the step (with `status`, so retries can match) unless `allow_error_status`. Only public addresses are reachable unless the operator allows private ones; cloud metadata addresses never are. |
| `core.notify` | Notifies `user:<id>`, `team:<id>`, `role:<key>` (holders at the run's organization), in the app and by email. Sent exactly once. |
| `integration.action` | Calls an action of an imported integration: `action: <integration>/<action>`, inputs under `with` (parameters by name, the request body as `body`), and optionally `connection: <key>`. Output: the response body. See [integrations.md](integrations.md#calling-actions). |
| `subworkflow` | Runs another published workflow (found by key here or in an organization above) and, by default, waits for its outputs. |
| `transform` | Computes a value. |
| `set_var` | Sets run variables, read as `run.vars.<name>`. |
| `approval` | Pauses until one of the `approvers` (`user:<id>`, `team:<id>`, `role:<key>` at the run's organization) approves or rejects under **Approvals**. They're notified with a link. The first decision counts. Output: `{decision: approved \| rejected, by: {id, name}, comment, decided_at}`. `expires` (default `7d`) fails it with `kind: timeout`. |
| `wait_for_callback` | Pauses until another system calls a single-use URL, then continues with the request (`method`, `query`, `content_type`, `body`) as output. `send` tells the other system the URL (`{{ callback.url }}`) with an integration action (`action`, `with`) or an HTTP request (`request`). `expires` (default `24h`) is how long it waits before failing with `kind: timeout`. Needs `auth.public_url`. |
| `wait` | Pauses for a `duration` or `until` a time. |
| `if`, `switch` | Run one block depending on a condition or a value. |
| `foreach` | Runs its steps for each item (`as` names the item; `concurrency` runs several at once). Output: the last step's output for each item. |
| `parallel` | Runs branches at once; `join: any` finishes with the first to succeed. |
| `try` | Runs `catch` if its steps fail (with `error`), and `finally` either way. |
| `return`, `fail` | End the run with outputs, or with an error. |

Every step has an `id` (unique in the workflow; others read its result as `steps.<id>.output`) and can have `if` (skip unless true), `timeout`, `retry` (`max_attempts`, `backoff: fixed | exponential | jitter`, `delay`, `max_delay`, `on`: statuses like `429`, `4xx`, `5xx`, `timeout`, `network`), and `continue_on_error`. `spec.on_error` runs when nothing caught a failure. `spec.settings` takes a run `timeout` and a `concurrency` limit (runs beyond it wait their turn).

## Variables and secrets

Organization variables (on the organization's page) are inherited down the tree; a nearer organization can set its own, and an organization can lock a variable for those below it. Workflows read them as `vars.<key>`. Secret variables are encrypted, never shown again, and read only as `secrets.<key>`: any step input that used one is stored redacted (the real value is encrypted for the step to use), and secret values are masked in what such steps return.

## Runs

The run page shows each step as it starts and finishes, with its input, output or error and attempts. Inputs and outputs over 256 KB (configurable) are kept in object storage: the page shows their size and a download link, and `GET /api/v1/runs/{id}/steps/{key}/{attempt}/{input|output}` returns them; expressions see the full value as usual. A failed run can be **resumed** after fixing the cause: steps that succeeded aren't run again. Running runs can be **cancelled**: nothing new starts, and subworkflows they started stop too. All runs are under **Runs**.

## Identity

A run started by hand acts as whoever started it. Runs started by a schedule, webhook or event act as the workflow's **run-as** service account (set on the workflow page), or as no one if none is set; subworkflows act as their parent. The identity is `actor.id` in expressions and is recorded on the run. Setting it needs `workflow:publish` and `service_account:manage`, since whoever hands out an account's identity could act as it. If the account is disabled, triggers start nothing (a webhook answers 409) until it's re-enabled or changed.

## Scripts

**Scripts** is a library of PowerShell 7 and Python 3 scripts. Each save is a new, numbered version, and earlier versions stay readable. Every script has a JSON Schema of its parameters, which a step running it will ask for. A library at an organization is shared with the organizations below it.

Running scripts is next. The `script` step will send them to sandboxed runners: rootless Podman containers by default, or Kubernetes Jobs (design doc §6.9). It needs `script:read` to view scripts and `script:manage` to change them.

## Permissions

| Permission | Allows |
|---|---|
| `workflow:read` | Viewing workflows, runs and variables |
| `workflow:write` | Creating and editing drafts, test runs |
| `workflow:publish` | Publishing, archiving, restoring |
| `workflow:run` | Starting, cancelling and resuming runs of published versions |
| `variable:manage` | Setting variables and secrets |

All are granted per organization and inherited by the organizations below. Technicians have read and run; read-only users have read.
