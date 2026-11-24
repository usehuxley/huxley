/**
 * The canvas layout (ADR-034): a workflow as nodes and edges, top to bottom
 * or left to right.
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

/** Which way steps follow each other. */
export type Direction = 'down' | 'right';

/**
 * Sizes along the flow (from one step to the next) and across it (where
 * lanes sit side by side). Left to right needs more room between steps:
 * lane labels and "+" sit on horizontal edges.
 */
interface Axes {
	direction: Direction;
	nodeAlong: number;
	nodeAcross: number;
	gapAlong: number;
	gapAcross: number;
}

const AXES: Record<Direction, Axes> = {
	down: {
		direction: 'down',
		nodeAlong: NODE_HEIGHT,
		nodeAcross: NODE_WIDTH,
		gapAlong: GAP_Y,
		gapAcross: GAP_X
	},
	right: {
		direction: 'right',
		nodeAlong: NODE_WIDTH,
		nodeAcross: NODE_HEIGHT,
		gapAlong: 120,
		gapAcross: GAP_X
	}
};

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

/** Room a step or list takes: across the flow, and along it. */
interface Size {
	across: number;
	along: number;
}

/** What a step needs to draw itself (and its lanes). */
function measureStep(step: Step, path: Path, selectedKey: string | null, axes: Axes): Size {
	const lanes = lanesOf(step, path, pathKey(path) === selectedKey);
	if (lanes.length === 0) return { across: axes.nodeAcross, along: axes.nodeAlong };
	const sizes = lanes.map((lane) => measureList(lane.steps, lane.path, selectedKey, axes));
	const lanesAcross =
		sizes.reduce((sum, s) => sum + s.across, 0) + axes.gapAcross * (sizes.length - 1);
	const lanesAlong = Math.max(...sizes.map((s) => s.along));
	return {
		across: Math.max(axes.nodeAcross, lanesAcross),
		along: axes.nodeAlong + axes.gapAlong + lanesAlong + axes.gapAlong + MARKER
	};
}

function measureList(steps: unknown[], path: Path, selectedKey: string | null, axes: Axes): Size {
	const sizes = steps.flatMap((step, i) =>
		isStep(step) ? [measureStep(step, [...path, i], selectedKey, axes)] : []
	);
	// An empty lane holds a placeholder, so its "+" has a place of its own.
	if (sizes.length === 0) return { across: axes.nodeAcross, along: MARKER };
	return {
		across: Math.max(axes.nodeAcross, ...sizes.map((s) => s.across)),
		along: sizes.reduce((sum, s) => sum + s.along, 0) + axes.gapAlong * (sizes.length - 1)
	};
}

class Builder {
	nodes: CanvasNode[] = [];
	edges: CanvasEdge[] = [];
	constructor(
		private readonly selectedKey: string | null,
		private readonly readonly: boolean,
		private readonly axes: Axes
	) {}

	/** A canvas position from one along the flow and one across it. */
	at(along: number, across: number): { x: number; y: number } {
		return this.axes.direction === 'down' ? { x: across, y: along } : { x: along, y: across };
	}

	/** A marker (start/end/join/empty) centred on `centre` across the flow. */
	marker(id: string, kind: 'end' | 'join' | 'empty', along: number, centre: number) {
		this.nodes.push({ id, kind, ...this.at(along, centre - MARKER / 2) });
	}

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
			y = bottom + this.axes.gapAlong;
			// A control-flow step's own exit is its join marker.
			if (lanesOf(step, path, id === this.selectedKey).length > 0) previous = `${id}:join`;
		});
		return {
			exit: previous,
			exitInsert: insert,
			bottom: y - (y === top ? 0 : this.axes.gapAlong)
		};
	}

	/** Places a step (and its lanes); returns its bottom edge. */
	step(step: Step, path: Path, centre: number, top: number): number {
		const id = pathKey(path);
		const { nodeAlong, nodeAcross, gapAlong, gapAcross } = this.axes;
		this.nodes.push({ id, kind: 'step', ...this.at(top, centre - nodeAcross / 2), path, step });
		const lanes = lanesOf(step, path, id === this.selectedKey);
		if (lanes.length === 0) return top + nodeAlong;

		const sizes = lanes.map((lane) =>
			measureList(lane.steps, lane.path, this.selectedKey, this.axes)
		);
		const total = sizes.reduce((sum, s) => sum + s.across, 0) + gapAcross * (sizes.length - 1);
		const lanesTop = top + nodeAlong + gapAlong;
		const lanesBottom = lanesTop + Math.max(...sizes.map((s) => s.along));
		const join = `${id}:join`;
		const joinTop = lanesBottom + gapAlong;
		let left = centre - total / 2;
		const exits: { exit: string; insert?: Insert }[] = [];
		lanes.forEach((lane, i) => {
			const laneCentre = left + sizes[i].across / 2;
			left += sizes[i].across + gapAcross;
			if (lane.steps.some(isStep)) {
				const placed = this.list(lane, laneCentre, lanesTop, id, lane.label);
				exits.push({ exit: placed.exit, insert: placed.exitInsert });
				return;
			}
			// An empty lane: a placeholder in its own column carries its
			// label and its "+", so lanes never share one.
			const empty = `${pathKey(lane.path)}:empty`;
			this.marker(empty, 'empty', lanesTop, laneCentre);
			this.edge(
				id,
				empty,
				{ list: lane.path, index: 0, label: `Add a step at the start of ${lane.label}` },
				lane.label
			);
			exits.push({ exit: empty });
		});
		this.marker(join, 'join', joinTop, centre);
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
 * Lays out `document`: its steps, and beside them (to the right, or below
 * when left to right) what runs if the run fails. `selectedKey` is the
 * selected step's path key (optional blocks of a selected step show even
 * while empty).
 */
export function layout(
	document: Record<string, unknown>,
	selectedKey: string | null,
	readonly = false,
	direction: Direction = 'down'
): Layout {
	const spec = isStep(document.spec) ? document.spec : {};
	const axes = AXES[direction];
	const { nodeAlong, nodeAcross, gapAlong, gapAcross } = axes;
	const builder = new Builder(selectedKey, readonly, axes);

	const main: Lane = { label: 'Steps', path: ['spec', 'steps'], steps: asList(spec.steps) };
	const mainSize = measureList(main.steps, main.path, selectedKey, axes);
	const centre = mainSize.across / 2;
	builder.nodes.push({
		id: 'start',
		kind: 'start',
		...builder.at(0, centre - nodeAcross / 2),
		title: 'Start',
		detail: triggerSummary(document)
	});
	const placed = builder.list(main, centre, nodeAlong + gapAlong, 'start');
	const endTop = (placed.exit === 'start' ? nodeAlong : placed.bottom) + gapAlong;
	builder.marker('end', 'end', endTop, centre);
	builder.edge(placed.exit, 'end', placed.exitInsert);

	const failure: Lane = {
		label: 'If the run fails',
		path: ['spec', 'on_error'],
		steps: asList(spec.on_error)
	};
	const failureSize = measureList(failure.steps, failure.path, selectedKey, axes);
	const failureCentre = mainSize.across + gapAcross * 3 + failureSize.across / 2;
	builder.nodes.push({
		id: 'on_error',
		kind: 'start',
		...builder.at(0, failureCentre - nodeAcross / 2),
		title: 'If the run fails',
		detail: 'when nothing else caught the error'
	});
	const failed = builder.list(failure, failureCentre, nodeAlong + gapAlong, 'on_error');
	const failedTop = (failed.exit === 'on_error' ? nodeAlong : failed.bottom) + gapAlong;
	builder.marker('on_error:end', 'end', failedTop, failureCentre);
	builder.edge(failed.exit, 'on_error:end', failed.exitInsert);

	return { nodes: builder.nodes, edges: builder.edges };
}
