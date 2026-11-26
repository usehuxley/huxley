# TypeScript SDK

`@usehuxley/sdk` writes workflows in TypeScript. They compile to HWF, the same format the visual editor saves, so they run the same way, open on the canvas, and sync with `huxley workflows` (ADR-016). The engine only ever runs HWF.

HWF `usehuxley.io/v1` is frozen: later changes only add (ADR-042). The SDK's types are generated from the published schema, so a workflow that compiles today keeps compiling and running.

## A workflow

```ts
// greet.workflow.ts
import { defineInputs, input, ref, step, text, trigger, when, workflow } from '@usehuxley/sdk';

const inputs = defineInputs({
	name: input.string({ title: 'Who to greet' }),
	loud: input.boolean({ default: false }).optional()
});

export default workflow({
	key: 'greet',
	name: 'Greet someone',
	triggers: [trigger.manual(), trigger.schedule({ cron: '0 9 * * 1-5', inputs: { name: 'team' } })],
	inputs: inputs.schema,
	steps: [
		step('say', 'transform', { output: text`Hello, ${inputs.refs.name}` }),
		when('shout', inputs.refs.loud, [
			step('tell', 'core.notify', {
				with: { to: 'role:technician', title: ref.steps.say.output, message: 'From the SDK' }
			})
		])
	],
	outputs: { line: ref.steps.say.output }
});
```

- **`step(id, type, fields)`** makes any step. `type` is one of HWF's step types, and `fields` is typed for it, so your editor completes and checks them.
- **Control flow:** `when` (if), `choose` (switch), `forEach`, `parallel` and `attempt` (try, with `catch` and `finally`).
- **Triggers:** `trigger.manual()`, `trigger.schedule({ cron | interval })`, or `trigger.of(type, fields)` for any trigger type.
- **Inputs:** `defineInputs({ … })` with `input.string | integer | number | boolean | array | object`. Inputs are required unless `.optional()`. It returns the JSON Schema and a reference to each input.

## Expressions

Steps read each other through `{{ … }}` expressions ([Expressions](expressions.md)). The SDK writes them for you:

| Write | Becomes |
|---|---|
| `ref.steps.fetch.output.body.items[0]` | `{{ steps.fetch.output.body.items[0] }}` |
| `ref.at('person').email` (a loop variable) | `{{ person.email }}` |
| ``text`Hello ${ref.inputs.name}` `` (or a template string) | `Hello {{ inputs.name }}` |
| ``expr`${ref.steps.find.output.people} \| length` `` | `{{ steps.find.output.people \| length }}` |

A reference used as a whole value becomes `{{ path }}`. Inside `expr`, references are bare paths in one expression, and other values are inlined as JSON. Raw `{{ … }}` text works anywhere too.

## Forms

`form({ key, name, fields })` with `field.text | email | date | choice | choices | number | yesNo` makes a pack form (`forms/<key>.json`). Fields can be shown only for certain answers (`showIf`), offered as an Action (`actionFor`), or offered in the client portal (`portal`, `portalCategory`).

## Building and publishing

Name files `*.workflow.ts` (one workflow, or a list, as the default export) and `*.form.ts`. Then:

```sh
huxley-sdk build src --out dist --forms dist-forms   # TypeScript → HWF JSON
huxley workflows validate dist                       # check them
huxley workflows test dist                           # fixture tests (*.test.yaml next to them)
huxley workflows push dist --publish                 # sync with the server
```

- **Node 24 runs the TypeScript directly:** no compile step or bundler is needed.
- **The output is deterministic:** the same sources always give the same bytes, so builds diff cleanly in Git.
- **Folders:** workflows keep their source folders, so `push` files them into the same [folders](workflows.md#folders). Each file is named by its workflow's key, and the same key twice is an error.
- **Git sync:** CI can build and push in the same pipeline ([Publishing from Git](workflows.md#publishing-from-git)). Add `pnpm install` and `pnpm build` before `validate`.

## Examples

[`examples/sdk`](../examples/sdk) has the built-in timesheet reminders pack written with the SDK. A test checks that they compile to exactly the pack's YAML. Copy the folder to start a repository of your own.
