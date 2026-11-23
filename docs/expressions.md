# Expressions

Huxley workflows use Jinja expressions, evaluated by [minijinja](https://github.com/mitsuhiko/minijinja) in a sandbox (ADR-005, design doc §6.3).

## Typed values and templates

- A string that is exactly one `{{ … }}` produces a **typed** value: `"{{ steps.lookup.output.items }}"` is a list, `"{{ inputs.count > 2 }}"` is a boolean.
- Any other string with Jinja renders to a string: `"Hello {{ inputs.name }}"`.
- Objects and lists are evaluated field by field. Numbers, booleans and strings without Jinja pass through unchanged.

Reading a missing key gives an undefined value: `null` as a result, false in a condition. Reading *through* it (`steps.x.output.missing.deeper`) is an error, and so is printing an undefined variable. Use `is defined`, `| default(…)` or `| default_if_none(…)` when something may be absent.

## What's in scope

| Name | What |
|---|---|
| `inputs` | The run's inputs |
| `trigger` | The trigger payload (webhook body, event, schedule) |
| `steps.<id>.output` | A completed step's output |
| `vars` | Organization variables (inherited down the org tree) |
| `secrets.<name>` | Secret values; see below |
| `org`, `actor`, `run`, `env`, `connections` | The running org, who started the run, run metadata, environment, connections |
| `item`, `loop` | Inside a `foreach`: the current item (renamed with `as:`) and `loop.index` etc. |
| `error` | Inside `catch` and `on_error`: the error being handled |

Before a workflow is published, every expression is compiled and checked: unknown names, and `steps.<id>` references to steps that can't have run yet (later steps, the step's own ancestors, the other side of an `if`/`switch`/`parallel`, or steps inside a loop from outside it; read `steps.<loop>.output` instead) are reported with their location.

## Filters and functions

Everything in Jinja's builtins (`lower`, `join`, `map`, `selectattr`, `tojson`, `default`, …), Python-style methods (`name.upper()`, `text.split(',')`, `d.items()`), `now()` with `datetimeformat`/`dateformat`/`timeformat`, plus:

| Filter / function | Example |
|---|---|
| `from_json`, `to_json(pretty=false)` | `"{{ body \| from_json }}"` |
| `b64encode(urlsafe=false)`, `b64decode` | `"Basic {{ (user ~ ':' ~ pass) \| b64encode }}"` |
| `hash(algorithm='sha256')` | `sha256` or `sha512`, hex |
| `slugify` | `"Hello World"` → `hello-world` |
| `regex_match`, `regex_search`, `regex_replace`, `regex_findall` | `"{{ subject \| regex_search('#[0-9]+') }}"` |
| `default_if_none(value)` | Replaces `null` as well as undefined |
| `uuid()` | A new UUIDv7 |
| `secrets.generate(length=24)` | A random password (8–256 characters) |

Regular expressions use Rust's `regex` syntax (linear time; no backreferences).

## Secrets

Reading `secrets.<name>` or calling `secrets.generate()` marks the evaluation as tainted: its result is passed to the step as is, but stored in run history and logs only as `«redacted»`. Known secret values are also masked wherever they appear in recorded inputs and outputs. The `secrets` object itself prints as `«redacted»` and can't be iterated.

## Limits

Each evaluation is bounded by fuel (instructions), recursion depth, and a 1 MiB result. There is no template loading, filesystem or network access.
