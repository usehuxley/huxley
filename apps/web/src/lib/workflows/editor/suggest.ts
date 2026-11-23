/**
 * Expression autocomplete for the visual editor: the names a step's
 * expressions can read (docs/expressions.md), and what to offer at the
 * caret. Step visibility follows the server's checker: a step sees the
 * steps before it in each enclosing block and what they contain, but not
 * the inside of a loop it isn't in.
 */

import { isStep, stepSpec, type Path, type Step } from './catalog.ts';

export interface Suggestion {
	/** What gets inserted, e.g. `steps.lookup.output`. */
	label: string;
	detail: string;
}

/** Filters and functions worth offering after `|` or on their own. */
export const FILTERS: Suggestion[] = [
	['default', 'A value if undefined'],
	['default_if_none', 'A value if undefined or null'],
	['length', 'Number of items or characters'],
	['lower', 'Lowercase'],
	['upper', 'Uppercase'],
	['trim', 'Without surrounding spaces'],
	['join', 'Join a list into text'],
	['map', 'Pick an attribute from each item'],
	['selectattr', 'Items whose attribute passes a test'],
	['first', 'The first item'],
	['last', 'The last item'],
	['tojson', 'As JSON text'],
	['from_json', 'Parse JSON text'],
	['b64encode', 'Base64-encode'],
	['b64decode', 'Base64-decode'],
	['hash', 'SHA-256 (or sha512) as hex'],
	['slugify', 'hello-world'],
	['regex_search', 'First match of a pattern'],
	['regex_replace', 'Replace a pattern'],
	['datetimeformat', 'Format a date and time']
].map(([label, detail]) => ({ label, detail }));

const FUNCTIONS: Suggestion[] = [
	{ label: 'now()', detail: 'The current time' },
	{ label: 'uuid()', detail: 'A new UUIDv7' },
	{ label: 'range(', detail: 'Numbers 0 to n-1' },
	{ label: 'secrets.generate()', detail: 'A random password' }
];

const GLOBALS: Suggestion[] = [
	{ label: 'trigger', detail: 'What started the run' },
	{ label: 'vars.', detail: 'Organization variables' },
	{ label: 'secrets.', detail: 'Secret organization variables' },
	{ label: 'run.id', detail: 'This run' },
	{ label: 'run.started_at', detail: 'When it started' },
	{ label: 'run.workflow.key', detail: 'The workflow' },
	{ label: 'org.id', detail: 'The organization it runs in' },
	{ label: 'actor.id', detail: 'Who it acts as' }
];

/** The step lists a step holds, with whether each is a loop body. */
function blocksOf(step: Step): { steps: unknown[]; loop: boolean }[] {
	const spec = stepSpec(step.type);
	const blocks: { steps: unknown[]; loop: boolean }[] = [];
	for (const block of spec?.blocks ?? []) {
		const value = step[block.path];
		if (Array.isArray(value)) blocks.push({ steps: value, loop: step.type === 'foreach' });
	}
	if (spec?.lanes) {
		const lanes = step[spec.lanes];
		if (Array.isArray(lanes)) {
			for (const lane of lanes) {
				if (isStep(lane) && Array.isArray(lane.steps))
					blocks.push({ steps: lane.steps, loop: false });
			}
		}
	}
	return blocks;
}

/** Ids in `steps` and inside them, leaving out loop bodies. */
function reachable(steps: unknown[], out: string[]) {
	for (const step of steps) {
		// Lanes (switch cases, parallel branches) aren't steps.
		if (!isStep(step) || typeof step.type !== 'string') continue;
		if (typeof step.id === 'string') out.push(step.id);
		for (const block of blocksOf(step)) if (!block.loop) reachable(block.steps, out);
	}
}

/** `set_var` names anywhere in the document, read as `run.vars.<name>`. */
function runVars(value: unknown, out: Set<string>) {
	if (Array.isArray(value)) value.forEach((v) => runVars(v, out));
	else if (isStep(value)) {
		if (value.type === 'set_var' && isStep(value.vars)) {
			Object.keys(value.vars).forEach((k) => out.add(k));
		}
		Object.values(value).forEach((v) => runVars(v, out));
	}
}

/** Everything an expression in the step at `path` can read. */
export function namesFor(document: unknown, path: Path): Suggestion[] {
	const names: Suggestion[] = [];
	const spec = isStep(document) && isStep(document.spec) ? document.spec : {};

	const inputs =
		isStep(spec.inputs) && isStep(spec.inputs.properties) ? spec.inputs.properties : {};
	for (const key of Object.keys(inputs)) names.push({ label: `inputs.${key}`, detail: 'Input' });

	// Walk down the path: at each step list, the steps before ours.
	const visible: string[] = [];
	const loops: Step[] = [];
	let current: unknown = document;
	for (let i = 0; i < path.length; i++) {
		const part = path[i];
		const next: unknown = (current as Record<string | number, unknown> | null)?.[part];
		if (Array.isArray(current) && typeof part === 'number') {
			reachable(current.slice(0, part), visible);
			const step: unknown = current[part];
			// Inside this step's loop body (not the step itself)?
			if (isStep(step) && step.type === 'foreach' && i < path.length - 1) loops.push(step);
		}
		current = next;
	}
	for (const id of visible) {
		names.push({ label: `steps.${id}.output`, detail: 'Step output' });
		names.push({ label: `steps.${id}.status`, detail: 'Step status' });
	}
	for (const loop of loops) {
		const item = typeof loop.as === 'string' && loop.as ? loop.as : 'item';
		names.push({ label: item, detail: `Current item of ${String(loop.id)}` });
		names.push({ label: 'loop.index', detail: 'Iteration number, from 1' });
	}
	const vars = new Set<string>();
	runVars(spec.steps, vars);
	for (const name of vars) names.push({ label: `run.vars.${name}`, detail: 'Run variable' });
	return [...names, ...GLOBALS, ...FUNCTIONS];
}

export interface Completion {
	/** The text to replace: `[from, to)` of the field's value. */
	from: number;
	to: number;
	items: Suggestion[];
}

/**
 * What to offer at `caret`: names matching the word being typed inside an
 * open `{{`, or filters after `|`. `null` outside an expression.
 */
export function complete(text: string, caret: number, names: Suggestion[]): Completion | null {
	const before = text.slice(0, caret);
	const open = before.lastIndexOf('{{');
	if (open < 0 || before.indexOf('}}', open) >= 0) return null;
	const word = /[A-Za-z_][\w.]*$/.exec(before)?.[0] ?? '';
	const from = caret - word.length;
	const afterPipe = /\|\s*$/.test(before.slice(open + 2, from));
	const pool = afterPipe ? FILTERS : names;
	const lower = word.toLowerCase();
	const items = pool
		.filter((s) => s.label.toLowerCase().startsWith(lower) && s.label !== word)
		.slice(0, 8);
	if (items.length === 0 || (!word && !afterPipe && !/\{\{\s*$/.test(before))) return null;
	return { from, to: caret, items };
}
