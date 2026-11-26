# ADR-060: One CodeMirror 6 editor, loaded on demand

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Scripts, expressions, workflow YAML and JSON Schemas were edited in plain text boxes, with no highlighting, completion or error marking. The user asked for a real code editor, suggesting CodeMirror 6 because Monaco is too heavy and slow (design doc §6.7).

## Decision

- **CodeMirror 6**, behind one component, `CodeEditor` in `@usehuxley/ui`. Every page that edits code uses it.
- **Alternatives considered:**
  - Monaco: several megabytes, and weak on phones.
  - Ace: older, weaker accessibility.
  - Very small editors such as prism-code-editor: no completion or lint framework.
  - CodeMirror is modular, MIT-licensed throughout, works with screen readers, and its extension system covers completion, linting, themes and read-only modes.
- **Loaded on demand:**
  - The component renders a plain textarea (which works without JavaScript and keeps forms submittable), then imports the editor when it mounts. Each language loads the first time an editor uses it.
  - Pages without code never download CodeMirror. The core is about 300 KB before compression, in its own chunk.
- **Languages:** PowerShell (CodeMirror's legacy mode), Python, JavaScript, TypeScript, YAML, JSON, Markdown and Jinja (`@codemirror/lang-jinja`), plus plain text.
- **Completion:**
  - Each language's own completion, plus words the page provides (for scripts, the parameters as each language receives them: `$Name` in PowerShell, `inputs["name"]` in Python).
  - The word source is created once per editor, because CodeMirror matches completion results to their source by identity.
  - Jinja takes a scope: names in scope, and the properties under a dotted path. This is how expressions will complete `steps.<id>.output.`.
- **Diagnostics:** a page passes a `lint` function, debounced, whose problems are underlined and marked in the gutter. The scripts page checks the parameters JSON Schema as you type. Expression and HWF checks will call the server's own validators, so the editor and the API never disagree.
- **Expressions** (the workflow editor's expression and JSON fields):
  - the editor in Jinja, in a compact form without gutters
  - completion comes from the editor's existing scope logic (`namesFor` and `complete`: inputs, steps that have run by then, loop items, run variables, filters and functions), passed in as a `complete(text, pos)` hook
  - edits commit shortly after typing stops, on blur, and when the field goes away
  - each field is checked as you type by sending the draft document, with the field's text in place, to `POST /workflows/validate` (through the page's `check` route) and keeping that field's problems, so the message is the API's own
- **Non-breaking spaces:** browsers put them into editable text where a typed space would collapse (beside an auto-closed `}}`, say), and an expression with one doesn't parse. Typed and pasted ones become ordinary spaces.
- **Theme:** colours come from the design tokens. Syntax colours are new tokens (`--hx-code-*`), each readable on its surface in light and dark.
- **Keyboard:** Tab indents. Escape then Tab leaves the editor (CodeMirror's rule), so the keyboard is never trapped.
- **Height:** the height limit sits on CodeMirror's scroller, not its outer element. The outer element is a flex column, so limiting it alone let the gutters spill over the controls below.

## Consequences

- One place to improve editing everywhere.
- New CodeMirror packages are pinned exactly, at versions already published for a while (pnpm's minimum release age stays on).
