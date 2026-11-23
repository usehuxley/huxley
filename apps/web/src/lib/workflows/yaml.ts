import { parse, stringify } from 'yaml';

/** Largest document the editor accepts (the API's limit too). */
export const MAX_DOCUMENT_BYTES = 1024 * 1024;

// The database stores drafts as JSON, which doesn't keep key order. These
// put HWF's keys back in the order people write them.
const TOP = ['apiVersion', 'kind', 'metadata', 'spec', 'ui'];
const METADATA = ['key', 'name', 'description', 'tags'];
const SPEC = ['inputs', 'triggers', 'steps', 'outputs', 'on_error', 'settings'];
const STEP_FIRST = ['id', 'type', 'name', 'description', 'if'];
const STEP_LAST = [
	'then',
	'else',
	'cases',
	'default',
	'items',
	'as',
	'concurrency',
	'steps',
	'branches',
	'join',
	'catch',
	'finally',
	'retry',
	'timeout',
	'continue_on_error'
];
const TRIGGER_FIRST = ['type', 'id'];

function ordered(object: Record<string, unknown>, first: string[], last: string[] = []) {
	const keys = Object.keys(object);
	const middle = keys.filter((k) => !first.includes(k) && !last.includes(k)).sort();
	const result: Record<string, unknown> = {};
	for (const key of [
		...first.filter((k) => k in object),
		...middle,
		...last.filter((k) => k in object)
	]) {
		result[key] = object[key];
	}
	return result;
}

function isObject(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function orderStep(step: unknown): unknown {
	if (!isObject(step)) return step;
	const result = ordered(step, STEP_FIRST, STEP_LAST);
	for (const block of ['then', 'else', 'default', 'steps', 'catch', 'finally']) {
		if (Array.isArray(result[block])) result[block] = (result[block] as unknown[]).map(orderStep);
	}
	if (Array.isArray(result.cases)) {
		result.cases = (result.cases as unknown[]).map((c) =>
			isObject(c)
				? ordered({ ...c, steps: Array.isArray(c.steps) ? c.steps.map(orderStep) : c.steps }, [
						'when',
						'steps'
					])
				: c
		);
	}
	if (Array.isArray(result.branches)) {
		result.branches = (result.branches as unknown[]).map((b) =>
			isObject(b)
				? ordered({ ...b, steps: Array.isArray(b.steps) ? b.steps.map(orderStep) : b.steps }, [
						'id',
						'steps'
					])
				: b
		);
	}
	return result;
}

/** Puts a document's keys in the conventional HWF order. */
export function orderDocument(document: unknown): unknown {
	if (!isObject(document)) return document;
	const result = ordered(document, TOP);
	if (isObject(result.metadata)) result.metadata = ordered(result.metadata, METADATA);
	if (isObject(result.spec)) {
		const spec = ordered(result.spec, SPEC);
		if (Array.isArray(spec.steps)) spec.steps = (spec.steps as unknown[]).map(orderStep);
		if (Array.isArray(spec.on_error)) spec.on_error = (spec.on_error as unknown[]).map(orderStep);
		if (Array.isArray(spec.triggers)) {
			spec.triggers = (spec.triggers as unknown[]).map((t) =>
				isObject(t) ? ordered(t, TRIGGER_FIRST) : t
			);
		}
		result.spec = spec;
	}
	return result;
}

/** A document as YAML for the editor. */
export function toYaml(document: unknown): string {
	return stringify(orderDocument(document), { lineWidth: 0 });
}

/** Parses the editor's YAML. */
export function fromYaml(text: string): { document: unknown } | { error: string } {
	if (new TextEncoder().encode(text).length > MAX_DOCUMENT_BYTES) {
		return { error: 'Workflow documents are limited to 1 MiB.' };
	}
	try {
		// A low alias limit stops "billion laughs" documents.
		const document: unknown = parse(text, { maxAliasCount: 100, prettyErrors: true });
		return { document };
	} catch (e) {
		return { error: e instanceof Error ? e.message : 'The YAML could not be read.' };
	}
}

/** A new workflow's starting document. */
export function starter(key: string, name: string): Record<string, unknown> {
	return {
		apiVersion: 'usehuxley.com/v1',
		kind: 'Workflow',
		metadata: { key, name },
		spec: {
			inputs: {
				type: 'object',
				properties: { name: { type: 'string', title: 'Name' } }
			},
			triggers: [{ type: 'manual' }],
			steps: [{ id: 'greet', type: 'transform', output: "Hello {{ inputs.name or 'world' }}" }],
			outputs: { greeting: '{{ steps.greet.output }}' }
		}
	};
}

/** A key from a name: lowercase letters, digits and single hyphens. */
export function keyFromName(name: string): string {
	return name
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '')
		.replace(/^[^a-z]+/, '')
		.slice(0, 64);
}
