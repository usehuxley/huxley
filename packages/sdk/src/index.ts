/**
 * Huxley workflows in TypeScript (ADR-016). Builders produce HWF
 * (`usehuxley.io/v1`, frozen by ADR-042) documents; `huxley-sdk build`
 * writes them as files for `huxley workflows push`.
 */

import type { Branch, HuxleyWorkflowFormatV1, Metadata, Settings, Step, Trigger } from './hwf.ts';
import { isRef, pathOf, refAt, type Ref } from './ref.ts';

export type * from './hwf.ts';
export { expr, isRef, pathOf, ref, refAt, text, type Ref } from './ref.ts';

export type Workflow = HuxleyWorkflowFormatV1;
export type StepType = Step['type'];
/** A step type's own fields (everything but `id` and `type`). */
export type StepFields<T extends StepType> = Omit<Extract<Step, { type: T }>, 'id' | 'type'>;
export type TriggerType = Trigger['type'];
export type TriggerFields<T extends TriggerType> = Omit<Extract<Trigger, { type: T }>, 'type'>;

/** References become `{{ path }}`; undefined fields are left out. */
export function serialize(value: unknown): unknown {
	if (isRef(value)) return `{{ ${pathOf(value as Ref)} }}`;
	if (Array.isArray(value)) return value.map(serialize);
	if (value && typeof value === 'object') {
		const out: Record<string, unknown> = {};
		for (const [k, v] of Object.entries(value)) {
			if (v !== undefined) out[k] = serialize(v);
		}
		return out;
	}
	return value;
}

/** A step of any type: `step('notify', 'core.notify', { with: { … } })`. */
export function step<T extends StepType>(
	id: string,
	type: T,
	fields?: StepFields<T>
): Extract<Step, { type: T }> {
	return serialize({ id, type, ...(fields ?? {}) }) as Extract<Step, { type: T }>;
}

type Common = Pick<
	StepFields<'if'>,
	'description' | 'continue_on_error' | 'retry' | 'timeout' | 'if' | 'name'
>;

/** `if`: run `then` when `condition` holds, else `otherwise`. */
export function when(
	id: string,
	condition: string,
	then: Step[],
	otherwise?: Step[],
	more?: Common
): Extract<Step, { type: 'if' }> {
	return step(id, 'if', { ...more, condition, then, else: otherwise });
}

/** `switch`: run the steps of the case equal to `value`, else `otherwise`. */
export function choose(
	id: string,
	value: unknown,
	cases: [when: unknown, steps: Step[]][],
	otherwise?: Step[],
	more?: Common
): Extract<Step, { type: 'switch' }> {
	return step(id, 'switch', {
		...more,
		value,
		cases: cases.map(([w, steps]) => ({ when: w as Record<string, unknown>, steps })),
		default: otherwise
	} as StepFields<'switch'>);
}

/** `foreach`: run `steps` for each of `items`, as `as` (default `item`). */
export function forEach(
	id: string,
	items: unknown,
	steps: Step[],
	options?: { as?: string; concurrency?: number } & Common
): Extract<Step, { type: 'foreach' }> {
	const { as, concurrency, ...more } = options ?? {};
	return step(id, 'foreach', { ...more, items, as, concurrency, steps } as StepFields<'foreach'>);
}

/** `parallel`: run branches at once; `join: 'any'` carries on after the first. */
export function parallel(
	id: string,
	branches: Record<string, Step[]>,
	options?: { join?: 'all' | 'any' } & Common
): Extract<Step, { type: 'parallel' }> {
	const { join, ...more } = options ?? {};
	return step(id, 'parallel', {
		...more,
		branches: Object.entries(branches).map(([bid, steps]): Branch => ({ id: bid, steps })),
		join
	} as StepFields<'parallel'>);
}

/** `try`: run `steps`; on failure `catch` (with `error` in scope); always `finally`. */
export function attempt(
	id: string,
	steps: Step[],
	options?: { catch?: Step[]; finally?: Step[] } & Common
): Extract<Step, { type: 'try' }> {
	const { catch: onError, finally: always, ...more } = options ?? {};
	return step(id, 'try', { ...more, steps, catch: onError, finally: always } as StepFields<'try'>);
}

/** Triggers: `trigger.schedule({ cron: '0 9 * * *' })`, or `trigger.of('event', {…})`. */
export const trigger = {
	of<T extends TriggerType>(type: T, fields?: TriggerFields<T>): Extract<Trigger, { type: T }> {
		return serialize({ type, ...(fields ?? {}) }) as Extract<Trigger, { type: T }>;
	},
	manual(): Extract<Trigger, { type: 'manual' }> {
		return trigger.of('manual');
	},
	schedule(fields: TriggerFields<'schedule'>): Extract<Trigger, { type: 'schedule' }> {
		return trigger.of('schedule', fields);
	}
};

/** A run input's JSON Schema, and whether it's required. */
export interface InputDef {
	schema: Record<string, unknown>;
	required: boolean;
}

type InputOptions = { title?: string; description?: string; default?: unknown };

function inputOf(schema: Record<string, unknown>) {
	const def: InputDef & { optional(): InputDef } = {
		schema,
		required: true,
		optional: () => ({ schema, required: false })
	};
	return def;
}

/** Inputs, required unless `.optional()`. */
export const input = {
	string: (o: InputOptions & { enum?: string[]; format?: string; maxLength?: number } = {}) =>
		inputOf({ type: 'string', ...o }),
	integer: (o: InputOptions & { minimum?: number; maximum?: number } = {}) =>
		inputOf({ type: 'integer', ...o }),
	number: (o: InputOptions & { minimum?: number; maximum?: number } = {}) =>
		inputOf({ type: 'number', ...o }),
	boolean: (o: InputOptions = {}) => inputOf({ type: 'boolean', ...o }),
	array: (items: Record<string, unknown>, o: InputOptions = {}) =>
		inputOf({ type: 'array', items, ...o }),
	object: (o: InputOptions & { properties?: Record<string, unknown> } = {}) =>
		inputOf({ type: 'object', ...o })
};

/** The inputs' JSON Schema, and a typed reference to each. */
export function defineInputs<K extends string>(
	inputs: Record<K, InputDef>
): { schema: Record<string, unknown>; refs: Record<K, Ref> } {
	const entries = Object.entries(inputs) as [K, InputDef][];
	const required = entries.filter(([, d]) => d.required).map(([k]) => k);
	const schema: Record<string, unknown> = {
		type: 'object',
		properties: Object.fromEntries(entries.map(([k, d]) => [k, d.schema])),
		...(required.length > 0 ? { required } : {})
	};
	const refs = Object.fromEntries(entries.map(([k]) => [k, refAt(`inputs.${k}`)])) as Record<
		K,
		Ref
	>;
	return { schema, refs };
}

export interface WorkflowDefinition {
	key: string;
	name: string;
	description?: string;
	tags?: string[];
	triggers?: Trigger[];
	/** A JSON Schema, or `defineInputs(…).schema`. */
	inputs?: Record<string, unknown>;
	steps: Step[];
	outputs?: Record<string, unknown>;
	onError?: Step[];
	settings?: Settings;
}

/** A workflow document (HWF v1). */
export function workflow(def: WorkflowDefinition): Workflow {
	const metadata: Metadata = { key: def.key, name: def.name };
	if (def.description !== undefined) metadata.description = def.description;
	if (def.tags) metadata.tags = def.tags;
	return serialize({
		apiVersion: 'usehuxley.io/v1',
		kind: 'Workflow',
		metadata,
		spec: {
			triggers: def.triggers,
			inputs: def.inputs,
			steps: def.steps,
			outputs: def.outputs,
			on_error: def.onError,
			settings: def.settings
		}
	}) as Workflow;
}

export { form, field, type FormDefinition, type FormDocument } from './form.ts';
