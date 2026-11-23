# Forms

A form is a set of questions people answer to start workflows (design doc §6.8). Find forms under **Forms**: fill one in, or create and edit them with `form:manage`.

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

`title` is a field's label, and `required` lists the fields that must be answered. UI hints can set `order` (the fields' order) and, per field, `widget`, `help` and `placeholder`. The editor shows a live preview as you change the definition.

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
