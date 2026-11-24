# ADR-022: The visual workflow editor shows nested blocks, not a free-form graph

- **Status:** Superseded by ADR-034
- **Date:** 2026-11-23

## Context

The implementation plan describes the canvas as a SvelteFlow node graph: a palette, connecting nodes, auto-layout. ADR-020 then made HWF control flow structured: control-flow steps own nested blocks, and there are no free-form edges. A node graph would let people draw connections the format can't express, and would have to forbid or translate them; its layout would be computed from the structure anyway.

## Decision

The visual editor renders a workflow as it is: a vertical list of step cards, where `if`, `switch`, `foreach`, `parallel` and `try` steps hold labelled lanes (`then`/`else`, cases, the loop body, branches, `catch`/`finally`) containing their own lists. People insert steps with "+" buttons between cards and in empty lanes, select a card to edit it in a side panel generated from the step type, and reorder or delete from there. Problems from the server's diagnostics appear on the card they belong to. The editor and the YAML view edit the same document.

This is the layout Power Automate and Logic Apps use for structured flows. It needs no graph library, works with keyboard and screen readers like any list of buttons and forms, and can't produce an invalid structure.

## Consequences

- No SvelteFlow dependency. Pan and zoom, sticky notes and free positioning (the `ui` block of HWF) aren't needed for layout; `ui` stays available for notes and collapsed state later.
- Very wide `parallel` or `switch` steps scroll horizontally inside their card rather than spreading over a canvas.
