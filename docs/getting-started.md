# Your first workflow

This tutorial builds a workflow that greets someone, tests it, publishes it, and lets a form start it. You need an account with the Owner or Administrator role (or `workflow:write`, `workflow:publish` and `form:manage`).

## 1. Create the workflow

Open **Workflows**. Give it a name, for example *Greeter*, with the key `greeter`, and select **Create**.

A new workflow starts as a draft with one step. Open the **YAML** tab to see the whole document:

```yaml
apiVersion: usehuxley.io/v1
kind: Workflow
metadata:
  key: greeter
  name: Greeter
spec:
  inputs:
    type: object
    properties:
      name: { type: string, title: Name }
  triggers:
    - type: manual
  steps:
    - id: greet
      type: transform
      output: "Hello {{ inputs.name or 'world' }}"
  outputs:
    greeting: "{{ steps.greet.output }}"
```

The parts:

- **`inputs`:** a JSON Schema of what the workflow takes.
- **`triggers`:** what starts it.
- **`steps`:** run in order. Each step's result is `steps.<id>.output`.
- **`outputs`:** what the run returns.

The `{{ … }}` parts are [expressions](expressions.md).

## 2. Make it do more

Change the steps to look a person up before greeting them, and **Save draft**:

```yaml
  steps:
    - id: user
      type: http.request
      with: { method: GET, url: "https://jsonplaceholder.typicode.com/users/1" }
    - id: greet
      type: transform
      output: "Hello {{ inputs.name }}, meet {{ steps.user.output.body.name }}"
```

Problems are listed as you save, such as a step that reads `steps.nope`. A workflow with problems can't be published.

## 3. Test it

**Test run the draft** runs the draft as it is, without publishing anything. The run page shows each step's input, output and timing as it happens. If a step fails, fix the draft and use **Run again from here** on that step: the steps before it keep their results.

## 4. Publish it

Add release notes and select **Publish version 1**. Runs use published versions. Editing the draft afterwards changes nothing until you publish again, and every version is kept.

## 5. Start it from a form

Open **Forms** and create one with a `name` field, set to start the *Greeter* workflow. Anyone with `form:submit` can now fill it in, and each submission starts a run with the answers as inputs. For people outside Huxley, create a **public link**.

## What next

- **[Connect an integration](integrations.md):** call Microsoft 365, your PSA, or any API with an OpenAPI spec through `integration.action` steps.
- **[Run a script](runners.md):** set up a runner for PowerShell, Python or JavaScript.
- **[Keep workflows in Git](workflows.md#workflows-as-code):** with `huxley workflows` and the CI pipelines.
- **[Let AI help](assistant.md):** describe the workflow you want to the assistant.
