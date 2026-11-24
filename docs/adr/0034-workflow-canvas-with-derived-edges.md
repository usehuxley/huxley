# ADR-034: The visual workflow editor is a SvelteFlow canvas whose edges come from the document

- **Status:** Accepted (supersedes ADR-022)
- **Date:** 2026-11-25

## Context

ADR-022 replaced the planned SvelteFlow canvas with nested step cards, because a free-form node graph would let people draw connections that structured HWF (ADR-020) can't express. The maintainer pointed out that people who use automation tools expect a canvas, and that the cards felt complicated and unfamiliar. A canvas doesn't need free-form connections: what an edge means is up to the editor.

## Decision

The visual editor is a SvelteFlow canvas (`@xyflow/svelte`, pinned to the 1.x line until 2.x settles):

- **Edges are derived from the document, never drawn.**
  - A step leads to the next.
  - `if`, `switch`, `foreach`, `parallel` and `try` fan out into labelled lanes (Then/Else, When …, Branch …, For each item, If it fails, Afterwards) that rejoin at a join point below.
  - An empty lane has a placeholder in its own column.
  - Nodes have no connectable handles, and nodes can't be dragged.
- **Steps are added on the edges.** Every edge has a "+" (the way n8n, Zapier and Make work), and it opens the step picker in the side panel. So inserting can only produce valid structure, and the picker is never covered by the canvas.
- **Layout is computed from the structure** (`layout.ts`, a pure function with unit tests): top to bottom, lanes side by side, with "If the run fails" in its own column. The same workflow always looks the same, and no positions are saved. HWF's `ui` block stays free for notes later.
- **The rest of the editor is unchanged.** Selecting a node opens the same settings panel, problems show as a badge on their node, and the YAML tab edits the same draft.
- **Accessibility:** each step and each "+" is one button with the same accessible name as before ("Step greet, Transform", "Add a step after greet"). SvelteFlow's own node and edge focus is turned off, so there's one tab stop per control. The canvas pans to keep the selected step, and anything that gets keyboard focus, in view.
- **Opening view:** the canvas opens on the top of the workflow, centred, at 60–100% zoom (so it's readable). Zoom controls and a minimap help with large workflows.

## Consequences

- The editor looks and behaves like the automation tools people know, and still can't produce a document the format can't express.
- Very wide `parallel` or `switch` steps spread across the canvas instead of scrolling inside a card.
- `@xyflow/svelte` (MIT, with d3 parts under ISC) is a new dependency of the web app. The CMDB dependency map (Phase 6) will use it too.
