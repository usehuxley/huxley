/**
 * The canvas layout (ADR-034): a workflow as nodes and edges, top to bottom.
 *
 * Edges come from the document's structure (a step leads to the next; an
 * `if`, `switch`, `foreach`, `parallel` or `try` fans out into labelled
 * lanes that rejoin below it), so nobody draws a connection the format
 * can't express. Every edge carries the "+" that inserts a step there. The
 * layout depends only on the structure: the same workflow always looks the
 * same, and nothing about positions is saved.
 */
import { isStep, pathKey, stepSpec, type Path, type Step } from './catalog.ts';

export const NODE_WIDTH = 248;
export const NODE_HEIGHT = 76;
/** Room between nodes for an edge, its label and its "+". */
export const GAP_Y = 72;
export const GAP_X = 40;
/** Start, end and join markers. */
export const MARKER = 28;

/** Where a "+" inserts a step: index `index` of the list at `list`. */
export interface Insert {
	list: Path;
	index: number;
	/** The button's accessible name. */
	label: string;
}

export type CanvasNode =
	| { id: string; kind: 'step'; x: number; y: number; path: Path; step: Step }
	| { id: string; kind: 'start'; x: number; y: number; title: string; detail: string }
	| { id: string; kind: 'end' | 'join' | 'empty'; x: number; y: number };

export interface CanvasEdge {
	id: string;
	source: string;
	target: string;
	/** The lane this edge enters ("Then", "When …"), shown on it. */
	label?: string;
	/** Set unless read-only. */
	insert?: Insert;
}

export interface Layout {
	nodes: CanvasNode[];
	edges: CanvasEdge[];
}

/** A list of steps inside a step (or at the top). */
interface Lane {
	label: string;
	path: Path;
	steps: unknown[];
}

const asList = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);

/** The lanes a control-flow step holds, in display order. */
export function lanesOf(step: Step, path: Path, selected: boolean): Lane[] {
	const spec = stepSpec(step.type);
	const lanes: Lane[] = [];
	if (spec?.lanes === 'cases') {
		asList(step.cases).forEach((c, i) => {
			if (!isStep(c)) return;
			const when = typeof c.when === 'string' ? c.when : JSON.stringify(c.when ?? '');
			lanes.push({
				label: `When ${when}`,
				path: [...path, 'cases', i, 'steps'],
				steps: asList(c.steps)
			});
		});
	}
	if (spec?.lanes === 'branches') {
		asList(step.branches).forEach((b, i) => {
			if (!isStep(b)) return;
			lanes.push({
				label: `Branch ${String(b.id)}`,
				path: [...path, 'branches', i, 'steps'],
				steps: asList(b.steps)
			});
		});
	}
	for (const block of spec?.blocks ?? []) {
		// Optional blocks (else, finally) show once they have steps, or
		// while their step is selected (to add the first one).
		if (!Array.isArray(step[block.path]) && !selected) continue;
		lanes.push({
			label: block.label,
			path: [...path, block.path],
			steps: asList(step[block.path])
		});
	}
	return lanes;
}

interface Size {
	width: number;
	height: number;
}

/** What a step needs to draw itself (and its lanes). */
function measureStep(step: Step, path: Path, selectedKey: string | null): Size {
	const lanes = lanesOf(step, path, pathKey(path) === selectedKey);
	if (lanes.length === 0) return { width: NODE_WIDTH, height: NODE_HEIGHT };
	const sizes = lanes.map((lane) => measureList(lane.steps, lane.path, selectedKey));
	const lanesWidth = sizes.reduce((sum, s) => sum + s.width, 0) + GAP_X * (sizes.length - 1);
	const lanesHeight = Math.max(...sizes.map((s) => s.height));
	return {
		width: Math.max(NODE_WIDTH, lanesWidth),
		height: NODE_HEIGHT + GAP_Y + lanesHeight + GAP_Y + MARKER
	};
}

function measureList(steps: unknown[], path: Path, selectedKey: string | null): Size {
	const sizes = steps.flatMap((step, i) =>
		isStep(step) ? [measureStep(step, [...path, i], selectedKey)] : []
	);
	// An empty lane holds a placeholder, so its "+" has a place of its own.
	if (sizes.length === 0) return { width: NODE_WIDTH, height: MARKER };
	return {
		width: Math.max(NODE_WIDTH, ...sizes.map((s) => s.width)),
		height: sizes.reduce((sum, s) => sum + s.height, 0) + GAP_Y * (sizes.length - 1)
	};
}

class Builder {
	nodes: CanvasNode[] = [];
	edges: CanvasEdge[] = [];
	constructor(
		private readonly selectedKey: string | null,
		private readonly readonly: boolean
	) {}

	edge(source: string, target: string, insert: Insert | undefined, label?: string) {
		this.edges.push({
			id: `${source}->${target}`,
			source,
			target,
			...(label ? { label } : {}),
			...(insert && !this.readonly ? { insert } : {})
		});
	}

	/**
	 * Places a lane's steps from `top`, centred on `centre`, coming from
	 * `entry`. Returns the last node (to connect onwards) and the "+" for
	 * the edge that leaves it, plus where the lane ends.
	 */
	list(
		lane: Lane,
		centre: number,
		top: number,
		entry: string,
		entryLabel?: string
	): { exit: string; exitInsert: Insert; bottom: number } {
		let previous = entry;
		let previousLabel = entryLabel;
		let insert: Insert = {
			list: lane.path,
			index: 0,
			label: `Add a step at the start of ${lane.label}`
		};
		let y = top;
		lane.steps.forEach((step, i) => {
			if (!isStep(step)) return;
			const path = [...lane.path, i];
			const id = pathKey(path);
			this.edge(previous, id, insert, previousLabel);
			const bottom = this.step(step, path, centre, y);
			previous = id;
			previousLabel = undefined;
			insert = { list: lane.path, index: i + 1, label: `Add a step after ${String(step.id)}` };
			y = bottom + GAP_Y;
			// A control-flow step's own exit is its join marker.
			if (lanesOf(step, path, id === this.selectedKey).length > 0) previous = `${id}:join`;
		});
		return {
			exit: previous,
			exitInsert: insert,
			bottom: y - (y === top ? 0 : GAP_Y)
		};
	}

	/** Places a step (and its lanes); returns its bottom edge. */
	step(step: Step, path: Path, centre: number, top: number): number {
		const id = pathKey(path);
		this.nodes.push({ id, kind: 'step', x: centre - NODE_WIDTH / 2, y: top, path, step });
		const lanes = lanesOf(step, path, id === this.selectedKey);
		if (lanes.length === 0) return top + NODE_HEIGHT;

		const sizes = lanes.map((lane) => measureList(lane.steps, lane.path, this.selectedKey));
		const total = sizes.reduce((sum, s) => sum + s.width, 0) + GAP_X * (sizes.length - 1);
		const lanesTop = top + NODE_HEIGHT + GAP_Y;
		const lanesBottom = lanesTop + Math.max(...sizes.map((s) => s.height));
		const join = `${id}:join`;
		const joinTop = lanesBottom + GAP_Y;
		let left = centre - total / 2;
		const exits: { exit: string; insert?: Insert }[] = [];
		lanes.forEach((lane, i) => {
			const laneCentre = left + sizes[i].width / 2;
			left += sizes[i].width + GAP_X;
			if (lane.steps.some(isStep)) {
				const placed = this.list(lane, laneCentre, lanesTop, id, lane.label);
				exits.push({ exit: placed.exit, insert: placed.exitInsert });
				return;
			}
			// An empty lane: a placeholder in its own column carries its
			// label and its "+", so lanes never share one.
			const empty = `${pathKey(lane.path)}:empty`;
			this.nodes.push({ id: empty, kind: 'empty', x: laneCentre - MARKER / 2, y: lanesTop });
			this.edge(
				id,
				empty,
				{ list: lane.path, index: 0, label: `Add a step at the start of ${lane.label}` },
				lane.label
			);
			exits.push({ exit: empty });
		});
		this.nodes.push({ id: join, kind: 'join', x: centre - MARKER / 2, y: joinTop });
		for (const { exit, insert } of exits) this.edge(exit, join, insert);
		return joinTop + MARKER;
	}
}

function triggerSummary(document: Record<string, unknown>): string {
	const spec = isStep(document.spec) ? document.spec : {};
	const types = asList(spec.triggers).flatMap((t) =>
		isStep(t) && typeof t.type === 'string' ? [t.type] : []
	);
	return types.length > 0 ? types.join(', ') : 'manual';
}

/**
 * Lays out `document`: its steps, and (to the right) what runs if the run
 * fails. `selectedKey` is the selected step's path key (optional blocks of
 * a selected step show even while empty).
 */
export function layout(
	document: Record<string, unknown>,
	selectedKey: string | null,
	readonly = false
): Layout {
	const spec = isStep(document.spec) ? document.spec : {};
	const builder = new Builder(selectedKey, readonly);

	const main: Lane = { label: 'Steps', path: ['spec', 'steps'], steps: asList(spec.steps) };
	const mainSize = measureList(main.steps, main.path, selectedKey);
	const centre = mainSize.width / 2;
	builder.nodes.push({
		id: 'start',
		kind: 'start',
		x: centre - NODE_WIDTH / 2,
		y: 0,
		title: 'Start',
		detail: triggerSummary(document)
	});
	const placed = builder.list(main, centre, NODE_HEIGHT + GAP_Y, 'start');
	const endTop = (placed.exit === 'start' ? NODE_HEIGHT : placed.bottom) + GAP_Y;
	builder.nodes.push({ id: 'end', kind: 'end', x: centre - MARKER / 2, y: endTop });
	builder.edge(placed.exit, 'end', placed.exitInsert);

	const failure: Lane = {
		label: 'If the run fails',
		path: ['spec', 'on_error'],
		steps: asList(spec.on_error)
	};
	const failureSize = measureList(failure.steps, failure.path, selectedKey);
	const failureCentre = mainSize.width + GAP_X * 3 + failureSize.width / 2;
	builder.nodes.push({
		id: 'on_error',
		kind: 'start',
		x: failureCentre - NODE_WIDTH / 2,
		y: 0,
		title: 'If the run fails',
		detail: 'when nothing else caught the error'
	});
	const failed = builder.list(failure, failureCentre, NODE_HEIGHT + GAP_Y, 'on_error');
	const failedTop = (failed.exit === 'on_error' ? NODE_HEIGHT : failed.bottom) + GAP_Y;
	builder.nodes.push({
		id: 'on_error:end',
		kind: 'end',
		x: failureCentre - MARKER / 2,
		y: failedTop
	});
	builder.edge(failed.exit, 'on_error:end', failed.exitInsert);

	return { nodes: builder.nodes, edges: builder.edges };
}
