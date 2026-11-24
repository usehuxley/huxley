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
- **Problems:** shown as a badge on the step they belong to.

Connections follow from the order and nesting of the steps, so the canvas can't build a workflow the format doesn't allow. Scroll or drag to move around, zoom with the controls, and use the minimap for large workflows. The arrows in the corner switch the flow between **top to bottom** and **left to right**; your browser remembers the choice. The **YAML** tab edits the same draft.

## Lifecycle

- **Draft:** saved at any time, even with problems; the editor lists what would stop it publishing, with where each problem is.
- **Publish:** makes the draft the next immutable version, after checking its structure, every expression (unknown names, references to steps that can't have run yet), and every `integration.action` step (the integration exists for the workflow's organization and has the action). Runs always pin to a version, and to the integration versions current when they start.
- **Test run:** runs the draft without publishing it (needs edit rights).
- **Archive:** stops triggers and runs; history stays. Restore brings the published version's triggers back.
- **Compare** any two versions, or a version and the draft.

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
| `webhook` | `POST /hooks/{id}` arrives (the URL is on the workflow page and stays the same across versions). Auth: `hmac` (signature header, SHA-256/512/1, optional prefix and signed timestamp), `bearer`, or `basic`. The body becomes the inputs. `response: sync` waits (up to `timeout`, at most 5m) and answers with the outputs; `async` answers 202 with the run id. |
| `event` | A domain event matches `event` (e.g. `org.created`, or `org.*`) in the workflow's organization or below, and the `filter` expression is true. The run happens in the event's organization. |
| `poll` | At each `interval` (at least `1m`), calls an integration's list action (`action`, optional `connection`, `with` passed as-is) and starts a run for each item it hasn't seen: `on: new` (default) or `new_or_changed`. `items` is where the list is in the response (e.g. `value`), `item_id` what identifies an item (default `id`). The first check only records what's there. The item is `trigger.item`, and `trigger.change` is `new` or `changed`. A failing check shows on the workflow's Triggers card. |
| `form` | Someone submits the form with this `form` key. The answers are the inputs, and the run happens in the organization it was submitted for, acting as the submitter. See [forms.md](forms.md). |
| `mcp_tool` | Offers the published workflow as a tool in the [MCP server](mcp.md#your-workflows-as-tools) and the assistant, named `run_<key>`, with its inputs as the tool's input schema. Optional `description` (default: the workflow's). |

## Steps

| Type | What it does |
|---|---|
| `http.request` | Calls an API. Output: `{status, headers, body}`. 4xx/5xx fail the step (with `status`, so retries can match) unless `allow_error_status`. Only public addresses are reachable unless the operator allows private ones; cloud metadata addresses never are. |
| `core.notify` | Notifies `user:<id>`, `team:<id>`, `role:<key>` (holders at the run's organization), in the app and by email. Sent exactly once. |
| `integration.action` | Calls an action of an imported integration: `action: <integration>/<action>`, inputs under `with` (parameters by name, the request body as `body`), and optionally `connection: <key>`. Output: the response body. See [integrations.md](integrations.md#calling-actions). |
| `subworkflow` | Runs another published workflow (found by key here or in an organization above) and, by default, waits for its outputs. |
| `core.contact.find` | Finds contacts by `email` and/or `external_id` (`{ entra: "<object id>" }`) among the run's organization and those below it. Output: `{ contacts, total }`. Needs the [Clients and contacts](crm.md) module. |
| `core.contact.upsert` | Updates the contact matched by `match` (`email`, the default, or an external id's system such as `entra`) at `org_id` (default: the run's organization), or adds one: `first_name`, `last_name`, `email`, `phone`, `mobile`, `job_title`, `external_ids` (merged). Fields left out keep their values. Output: `{ contact, created }`. |
| `core.company.get` | An organization's company details and sites (`org_id`, default the run's). Output: `{ company, sites }`. |
| `core.company.update` | Changes company details (`status`, `relationship`, `website`, `phone`, `notes`); the rest stays. Output: `{ company }`. |
| `core.mail.fetch` | Fetches new mail from the enabled IMAP mailboxes at the run's organization and below (or just `mailbox_id`), up to `limit` per mailbox (default 50). Each new message is kept and raises `email.received`. A mailbox that can't be reached is reported, and its error kept on it, without failing the step. Output: `{ mailboxes: [{ mailbox_id, address, fetched, new } or { mailbox_id, address, error }] }`. Needs the [Service desk](servicedesk.md#email) module. |
| `core.ticket.email_reply` | Emails a public note (`note_id`, from `ticket.note_added`) to its ticket's requester, from the mailbox the ticket's mail came to (else the nearest at its organization or above). The subject has the ticket's `[HX#…]` token, and the message continues the email thread. Internal notes, notes from the requester, and notes already sent are skipped. Output: `{ sent: true, to, message_id }` or `{ sent: false, reason }`. Needs email set up for the instance. |
| `core.ticket.from_email` | Turns a received email (`email_id`, from `email.received`) into ticket work, following its mailbox's rules. It skips automatic mail and loops, adds a reply to the ticket it belongs to (reopening a resolved one), or opens a ticket for the sender, with the email's attachments on it. Output: `{ outcome, detail, ticket_id, created, attachments }`. Running it again for the same email changes nothing. Needs the [Service desk](servicedesk.md#email) module. |
| `transform` | Computes a value. |
| `set_var` | Sets run variables, read as `run.vars.<name>`. |
| `approval` | Pauses until one of the `approvers` (`user:<id>`, `team:<id>`, `role:<key>` at the run's organization) approves or rejects under **Approvals**. They're notified with a link. The first decision counts. Output: `{decision: approved \| rejected, by: {id, name}, comment, decided_at}`. `expires` (default `7d`) fails it with `kind: timeout`. |
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

**Scripts** is a library of PowerShell 7 and Python 3 scripts. Each save is a new, numbered version, and earlier versions stay readable. Every script has a JSON Schema of its parameters, which a step running it will ask for. A library at an organization is shared with the organizations below it.

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

`huxley workflows` keeps workflows in files (HWF YAML or JSON), for example in a Git repository, and syncs them with a Huxley server (design doc §6.7).

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
- **Matching:** files are matched to workflows by `metadata.key`.
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
