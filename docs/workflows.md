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
- **Publish:** makes the draft the next immutable version, after checking its structure and every expression (unknown names, references to steps that can't have run yet). Runs always pin to a version.
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
| `form`, `mcp_tool` | Reserved for forms (Phase 3) and MCP (Phase 4). |

## Steps

| Type | What it does |
|---|---|
| `http.request` | Calls an API. Output: `{status, headers, body}`. 4xx/5xx fail the step (with `status`, so retries can match) unless `allow_error_status`. Only public addresses are reachable unless the operator allows private ones; cloud metadata addresses never are. |
| `core.notify` | Notifies `user:<id>`, `team:<id>`, `role:<key>` (holders at the run's organization), in the app and by email. Sent exactly once. |
| `subworkflow` | Runs another published workflow (found by key here or in an organization above) and, by default, waits for its outputs. |
| `transform` | Computes a value. |
| `set_var` | Sets run variables, read as `run.vars.<name>`. |
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

## Permissions

| Permission | Allows |
|---|---|
| `workflow:read` | Viewing workflows, runs and variables |
| `workflow:write` | Creating and editing drafts, test runs |
| `workflow:publish` | Publishing, archiving, restoring |
| `workflow:run` | Starting, cancelling and resuming runs of published versions |
| `variable:manage` | Setting variables and secrets |

All are granted per organization and inherited by the organizations below. Technicians have read and run; read-only users have read.
