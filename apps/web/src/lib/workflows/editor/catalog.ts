/**
 * What the visual editor knows about each HWF step type: how to label it,
 * what a new one looks like, which fields its panel edits, and which nested
 * blocks it holds (ADR-020, ADR-022).
 */

export type FieldKind = 'text' | 'expression' | 'number' | 'boolean' | 'json' | 'select';

export interface FieldSpec {
	/** Dotted path within the step, e.g. `with.url`. */
	path: string;
	label: string;
	kind: FieldKind;
	hint?: string;
	options?: { value: string; label: string }[];
}

export interface StepSpec {
	type: string;
	label: string;
	description: string;
	group: 'Actions' | 'Data' | 'Control flow' | 'Finish';
	/** Fields of a new step, besides `id` and `type`. */
	template: Record<string, unknown>;
	fields: FieldSpec[];
	/** Nested step lists: fixed ones (`then`), or a list of lanes (`cases`). */
	blocks?: { path: string; label: string }[];
	lanes?: 'cases' | 'branches';
}

const methods = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'].map((m) => ({
	value: m,
	label: m
}));

export const STEPS: StepSpec[] = [
	{
		type: 'http.request',
		label: 'HTTP request',
		description: 'Call an API.',
		group: 'Actions',
		template: { with: { method: 'GET', url: 'https://' } },
		fields: [
			{ path: 'with.method', label: 'Method', kind: 'select', options: methods },
			{ path: 'with.url', label: 'URL', kind: 'expression' },
			{ path: 'with.headers', label: 'Headers', kind: 'json', hint: 'An object' },
			{ path: 'with.query', label: 'Query', kind: 'json', hint: 'An object' },
			{ path: 'with.body', label: 'Body', kind: 'json' },
			{
				path: 'with.body_type',
				label: 'Body type',
				kind: 'select',
				options: [
					{ value: 'json', label: 'JSON' },
					{ value: 'form', label: 'Form' },
					{ value: 'text', label: 'Text' }
				]
			},
			{ path: 'with.allow_error_status', label: 'Treat 4xx/5xx as success', kind: 'boolean' }
		]
	},
	{
		type: 'core.notify',
		label: 'Notify',
		description: 'Tell people in the app and by email.',
		group: 'Actions',
		template: { with: { to: 'role:admin', title: '' } },
		fields: [
			{
				path: 'with.to',
				label: 'To',
				kind: 'json',
				hint: 'user:<id>, team:<id>, role:<key>, or a list'
			},
			{ path: 'with.title', label: 'Title', kind: 'expression' },
			{ path: 'with.message', label: 'Message', kind: 'expression' },
			{ path: 'with.url', label: 'Link', kind: 'text', hint: 'A path in the app, e.g. /runs' }
		]
	},
	{
		type: 'subworkflow',
		label: 'Run workflow',
		description: 'Run another workflow and use its outputs.',
		group: 'Actions',
		template: { workflow: '' },
		fields: [
			{ path: 'workflow', label: 'Workflow key', kind: 'text' },
			{ path: 'with', label: 'Inputs', kind: 'json', hint: 'An object' },
			{ path: 'wait', label: 'Wait for it to finish', kind: 'boolean' }
		]
	},
	{
		type: 'transform',
		label: 'Transform',
		description: 'Compute a value with an expression.',
		group: 'Data',
		template: { output: '{{ }}' },
		fields: [
			{
				path: 'output',
				label: 'Output',
				kind: 'json',
				hint: 'Any value; strings may be expressions'
			}
		]
	},
	{
		type: 'set_var',
		label: 'Set variables',
		description: 'Set run variables (run.vars).',
		group: 'Data',
		template: { vars: {} },
		fields: [{ path: 'vars', label: 'Variables', kind: 'json', hint: 'An object' }]
	},
	{
		type: 'wait',
		label: 'Wait',
		description: 'Pause for a while or until a time.',
		group: 'Control flow',
		template: { duration: '5m' },
		fields: [
			{ path: 'duration', label: 'Duration', kind: 'text', hint: 'e.g. 30s, 5m, 1h30m' },
			{ path: 'until', label: 'Until', kind: 'expression', hint: 'Or an RFC 3339 time' }
		]
	},
	{
		type: 'if',
		label: 'If',
		description: 'Branch on a condition.',
		group: 'Control flow',
		template: { condition: '{{ }}', then: [], else: [] },
		fields: [{ path: 'condition', label: 'Condition', kind: 'expression' }],
		blocks: [
			{ path: 'then', label: 'Then' },
			{ path: 'else', label: 'Else' }
		]
	},
	{
		type: 'switch',
		label: 'Switch',
		description: 'Choose a branch by value.',
		group: 'Control flow',
		template: { value: '{{ }}', cases: [{ when: '', steps: [] }], default: [] },
		fields: [{ path: 'value', label: 'Value', kind: 'expression' }],
		lanes: 'cases',
		blocks: [{ path: 'default', label: 'Otherwise' }]
	},
	{
		type: 'foreach',
		label: 'For each',
		description: 'Repeat steps for every item of a list.',
		group: 'Control flow',
		template: { items: '{{ }}', steps: [] },
		fields: [
			{ path: 'items', label: 'Items', kind: 'expression' },
			{ path: 'as', label: 'Item name', kind: 'text', hint: 'Default: item' },
			{ path: 'concurrency', label: 'At once', kind: 'number', hint: '1 to 50' }
		],
		blocks: [{ path: 'steps', label: 'For each item' }]
	},
	{
		type: 'parallel',
		label: 'Parallel',
		description: 'Run branches at the same time.',
		group: 'Control flow',
		template: {
			branches: [
				{ id: 'a', steps: [] },
				{ id: 'b', steps: [] }
			],
			join: 'all'
		},
		fields: [
			{
				path: 'join',
				label: 'Finish when',
				kind: 'select',
				options: [
					{ value: 'all', label: 'All branches finish' },
					{ value: 'any', label: 'The first branch succeeds' }
				]
			}
		],
		lanes: 'branches'
	},
	{
		type: 'try',
		label: 'Try',
		description: 'Handle errors from a group of steps.',
		group: 'Control flow',
		template: { steps: [], catch: [] },
		fields: [],
		blocks: [
			{ path: 'steps', label: 'Try' },
			{ path: 'catch', label: 'If it fails' },
			{ path: 'finally', label: 'Afterwards' }
		]
	},
	{
		type: 'return',
		label: 'Return',
		description: 'Finish the run with outputs.',
		group: 'Finish',
		template: { outputs: {} },
		fields: [{ path: 'outputs', label: 'Outputs', kind: 'json', hint: 'An object' }]
	},
	{
		type: 'fail',
		label: 'Fail',
		description: 'Stop the run with an error.',
		group: 'Finish',
		template: { message: '' },
		fields: [
			{ path: 'message', label: 'Message', kind: 'expression' },
			{ path: 'code', label: 'Code', kind: 'text' }
		]
	}
];

/** Fields every step has. */
export const COMMON_FIELDS: FieldSpec[] = [
	{ path: 'name', label: 'Display name', kind: 'text' },
	{ path: 'if', label: 'Only if', kind: 'expression', hint: 'Skip the step unless this is true' },
	{ path: 'timeout', label: 'Timeout', kind: 'text', hint: 'e.g. 30s' },
	{ path: 'retry', label: 'Retry', kind: 'json', hint: '{ "max_attempts": 3, "on": ["5xx"] }' },
	{ path: 'continue_on_error', label: 'Continue if it fails', kind: 'boolean' }
];

export function stepSpec(type: unknown): StepSpec | undefined {
	return STEPS.find((s) => s.type === type);
}

export type Step = Record<string, unknown>;
/** Where a value sits in the document: keys and indices from the root. */
export type Path = (string | number)[];

export function isStep(value: unknown): value is Step {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function getAt(root: unknown, path: Path): unknown {
	let current: unknown = root;
	for (const part of path) {
		if (current === null || typeof current !== 'object') return undefined;
		current = (current as Record<string | number, unknown>)[part];
	}
	return current;
}

/** Reads a dotted field path (`with.url`) inside a step. */
export function getField(step: Step, field: string): unknown {
	return getAt(step, field.split('.'));
}

/** Sets a dotted field path, creating objects on the way; `undefined` removes it. */
export function setField(step: Step, field: string, value: unknown): void {
	const parts = field.split('.');
	let current: Record<string, unknown> = step;
	for (const part of parts.slice(0, -1)) {
		if (!isStep(current[part])) current[part] = {};
		current = current[part] as Record<string, unknown>;
	}
	const last = parts[parts.length - 1];
	if (value === undefined || value === '') delete current[last];
	else current[last] = value;
}

/** Every step id in the document, to pick unique new ones. */
export function stepIds(document: unknown): Set<string> {
	const ids = new Set<string>();
	const visit = (value: unknown) => {
		if (Array.isArray(value)) value.forEach(visit);
		else if (isStep(value)) {
			if (typeof value.id === 'string' && typeof value.type === 'string') ids.add(value.id);
			Object.values(value).forEach(visit);
		}
	};
	visit(document);
	return ids;
}

/** A new step of `type` with an id not used elsewhere in the document. */
export function newStep(type: string, document: unknown): Step {
	const spec = stepSpec(type)!;
	const base = type.replace(/[^a-z]+/g, '_').replace(/^_+|_+$/g, '');
	const taken = stepIds(document);
	let n = 1;
	while (taken.has(`${base}_${n}`)) n++;
	return { id: `${base}_${n}`, type, ...structuredClone(spec.template) };
}

/** `spec.steps[0].then[1]` as a path, for matching diagnostics to steps. */
export function pathKey(path: Path): string {
	return path
		.map((part, i) => (typeof part === 'number' ? `[${part}]` : i === 0 ? part : `.${part}`))
		.join('');
}

/** A short line describing what a step does, for its card. */
export function summary(step: Step): string {
	const text = (v: unknown) =>
		typeof v === 'string' ? v : v === undefined ? '' : JSON.stringify(v);
	switch (step.type) {
		case 'http.request':
			return `${text(getField(step, 'with.method') ?? 'GET')} ${text(getField(step, 'with.url'))}`;
		case 'core.notify':
			return `${text(getField(step, 'with.to'))}: ${text(getField(step, 'with.title'))}`;
		case 'subworkflow':
			return text(step.workflow);
		case 'transform':
			return text(step.output);
		case 'set_var':
			return Object.keys(isStep(step.vars) ? step.vars : {}).join(', ');
		case 'wait':
			return text(step.duration ?? step.until);
		case 'if':
			return text(step.condition);
		case 'switch':
			return text(step.value);
		case 'foreach':
			return text(step.items);
		case 'fail':
			return text(step.message);
		default:
			return '';
	}
}
