# Forms

A form is a set of questions people answer to start workflows (design doc §6.8). Find forms under **Forms**: fill one in, or create and edit them with `form:manage`.

## Building a form

The editor's **Fields** view builds a form without any JSON. For each question, choose:

- a label
- a type: text, long text, email, number, whole number, date, date and time, checkbox, choice, or multiple choice
- whether it's required
- help text and a placeholder
- the choices, for choice and multiple choice

Reorder the fields with the arrows. Each field has a **name in workflows**, which a workflow reads as `inputs.<name>`. It's made from the label until you change it, and uses lowercase letters, digits and underscores. A preview shows the form as people will see it.

**JSON** shows the definition behind the fields, for anything the builder doesn't cover, such as `minLength`, `pattern` or `minimum`. The builder keeps those settings when you switch back.

## Definition

A form is a **JSON Schema** of its answers (an object), plus **UI hints**.

| Field | Schema |
|---|---|
| Text | `{ "type": "string" }`, with `maxLength`, `pattern`, … |
| Long text | a string, with the hint `{ "widget": "textarea" }` |
| Email, date, date and time | a string with `format: email \| date \| date-time` |
| Choice | a string with `enum` |
| Several choices | `{ "type": "array", "items": { "type": "string", "enum": [...] } }` |
| Number, whole number | `number`, `integer`, with `minimum`, `maximum`, … |
| Yes / no | `boolean` |

`title` is a field's label, and `required` lists the fields that must be answered. UI hints can set `order` (the fields' order) and, per field, `widget`, `help` and `placeholder`. The editor shows a live preview as you change it.

## Actions

A form can be an **Action**: something technicians run on a client, a contact or a ticket, such as *Reset MFA* or *Add to a group*. In the form's editor, set **Offered as an Action on** to clients, contacts or tickets. A form that isn't an Action is filled in from **Forms** as usual.

**Where Actions show.** On a client's, contact's or ticket's page, the **Actions** card lists the Actions for that kind of record. It lists those owned by the record's organization or one above it. Choosing one opens the form for that record's organization, and says what it runs on.

**What workflows get.** The workflows it starts get the answers as `inputs`, and the record as `trigger.context`:

| `trigger.context.…` | |
|---|---|
| `client` | `id`, `name`, `kind` |
| `contact` | `id`, `name`, `first_name`, `last_name`, `email`, `phone`, `mobile`, `external_ids` (e.g. `entra`) |
| `ticket` | `id`, `reference`, `title`, `priority`, `status`, `type`, `queue`, `assignee` |

Run from a ticket, the contact is its requester, and the ticket's history records which Action was run. Huxley checks that the contact or ticket belongs to the organization the form is for, and that you can see it.

**From the API.** Through the API, send `context: { contactId, ticketId }` with the submission (`POST /api/v1/forms/{id}/submissions`). A pack's form can be an Action too, with `"actionFor"` in its JSON.

## In the client portal

Tick **Offer in the client portal** in the editor (or `"portal": true`, with an optional `"portalCategory"`, in a pack's form JSON) to list the form under **Request something** in the client portal. Contacts of the form's organization and those below it can fill it in. The runs act as each workflow's run-as account, and get the contact and their company as `trigger.context`. Such forms can't look choices up through an integration. See [the client portal](portal.md#requests).

## Secret answers

A **Secret (password)** field, which is `{ "type": "string", "writeOnly": true }` in JSON, is for answers such as a temporary password or a PIN. It shows as a password box. The stored submission keeps only `«redacted»`, and the runs it starts keep the answer encrypted, even if the workflow doesn't mark that input secret. Workflows read it as `inputs.<name>`, with the same masking as secrets (see [workflows](workflows.md#variables-and-secrets)).

## Questions that depend on answers

A field can be shown only when another field's answer is one of some values. In the builder, use **Show only when … is …**. In JSON, use the UI hint `"show_if": { "field": "kind", "equals": "existing" }`, or `"in": [...]` for several values; with no value, the field is shown once the other field is answered. A multiple-choice field matches if any of its answers does.

A hidden field isn't required, and **its answer is dropped**, also when what hides it is itself hidden. Huxley decides this again when the form is submitted, so a workflow never sees answers to questions the person wasn't shown.

## Choices from an integration

A choice field can take its choices from a **read-only (GET) integration action**, such as the users in a Microsoft 365 tenant. In the builder, tick **Choices from an integration**. In JSON, use the hint:

```json
"options_from": {
  "action": "msgraph/listUsers",
  "connection": "contoso",
  "with": { "$top": 100 },
  "items": "value",
  "value": "id",
  "label": "displayName"
}
```

- `items` is where the list is in the response, as a dot-separated path; leave it out if the response is the list.
- `value` and `label` are paths within each item.
- `connection` is optional; the default is the nearest connection the organization may use, as for actions.

The choices are looked up for the organization the form is being filled in for, with up to 1,000 per field. On submit, Huxley looks them up again, so an answer must be one of the current choices. Actions other than GET are refused, so filling a form in can't change anything.

## Public links

Under **Public links** on a form's edit page, make a link that anyone can use to fill the form in **without signing in**, for example on a client's intranet or website.

- **One organization:** each link is for one organization, the form's or one below it, and can have an expiry. The link (`/f/hxf_…`) is shown once; only a hash of it is kept. **Turn off** stops it at once.
- **Who the runs act as:** each workflow's **run-as service account** (or no one), as webhooks do, never a person. A workflow whose run-as account is disabled doesn't start.
- **What submitters see:** only that their answers were sent, nothing about the runs. Submissions record the link they came through.
- **Rate limit:** 30 requests a minute per client address.
- **No looked-up choices:** forms that look choices up through an integration can't have public links, because the lookup would show integration data to anyone with the link. While a form has active links, it can't be changed to look choices up.

Making and turning off links needs `form:manage`.

## Starting workflows

A workflow listens for a form with a trigger:

```yaml
triggers:
  - type: form
    form: laptop-request    # the form's key
```

When someone submits the form:

- The answers are checked against the form. Problems are shown next to their fields.
- Every published workflow in the **form's organization** with a trigger for the form starts, with the answers as its inputs.
- The runs happen in the organization the form was filled in **for**: the form's organization or one below it, such as a client. They act as the person who submitted it.
- A workflow whose inputs don't match the answers doesn't start, and the submitter is told which one.
- Each submission is kept with its runs. People who manage the form see recent submissions on its edit page.

## Permissions

| Permission | Allows |
|---|---|
| `form:submit` | Filling in forms (technicians have it) |
| `form:manage` | Creating, editing, archiving and restoring forms |

Not yet: the drag-and-drop builder, options loaded from integrations, conditional fields, password fields, and public or client-portal links.

Choices can also come from a [dataset](datasets.md): `options_from: { dataset, filters, value, label }`.
