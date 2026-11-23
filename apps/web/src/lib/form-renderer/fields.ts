/**
 * Turns a form's JSON Schema (and UI hints) into the fields to render:
 * the subset `huxley-forms` accepts.
 */

export type FieldKind =
	| 'text'
	| 'textarea'
	| 'email'
	| 'date'
	| 'datetime'
	| 'number'
	| 'integer'
	| 'boolean'
	| 'select'
	| 'multiselect';

/** When a field is shown (a `show_if` hint). */
export interface ShowIf {
	field: string;
	equals?: unknown;
	in?: unknown[];
}

export interface Choice {
	value: string;
	label: string;
}

export interface Field {
	name: string;
	label: string;
	kind: FieldKind;
	required: boolean;
	help?: string;
	placeholder?: string;
	/** Fixed choices (`enum`). */
	options: string[];
	showIf?: ShowIf;
	/** Its choices are looked up through an integration (`options_from`). */
	lookup: boolean;
}

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v);
const text = (v: unknown) => (typeof v === 'string' ? v : undefined);

function kindOf(schema: Obj, hint: Obj): FieldKind {
	if (isObj(hint.options_from)) return schema.type === 'array' ? 'multiselect' : 'select';
	switch (schema.type) {
		case 'boolean':
			return 'boolean';
		case 'integer':
			return 'integer';
		case 'number':
			return 'number';
		case 'array':
			return 'multiselect';
		default:
			if (Array.isArray(schema.enum)) return 'select';
			if (schema.format === 'email') return 'email';
			if (schema.format === 'date') return 'date';
			if (schema.format === 'date-time') return 'datetime';
			return hint.widget === 'textarea' ? 'textarea' : 'text';
	}
}

function showIfOf(v: unknown): ShowIf | undefined {
	if (!isObj(v) || typeof v.field !== 'string') return undefined;
	return {
		field: v.field,
		...('equals' in v ? { equals: v.equals } : {}),
		...(Array.isArray(v.in) ? { in: v.in as unknown[] } : {})
	};
}

function matches(rule: ShowIf, answer: unknown): boolean {
	const wanted = 'equals' in rule ? [rule.equals] : rule.in;
	if (wanted === undefined) return answer !== undefined && answer !== null && answer !== '';
	const given = Array.isArray(answer) ? answer : [answer];
	return given.some((g) => wanted.some((w) => w === g));
}

/**
 * The fields shown for these answers: a field whose `show_if` doesn't
 * match is hidden, and so is one whose rule depends on a hidden field
 * (as the server decides).
 */
export function shownFields(fields: Field[], values: Record<string, unknown>): Set<string> {
	const hidden = new Set<string>();
	for (let changed = true; changed;) {
		changed = false;
		for (const f of fields) {
			if (hidden.has(f.name) || !f.showIf) continue;
			if (hidden.has(f.showIf.field) || !matches(f.showIf, values[f.showIf.field])) {
				hidden.add(f.name);
				changed = true;
			}
		}
	}
	return new Set(fields.map((f) => f.name).filter((n) => !hidden.has(n)));
}

/** The form's fields, in `ui.order` (then schema) order. */
export function fieldsOf(schema: unknown, ui: unknown): Field[] {
	if (!isObj(schema) || !isObj(schema.properties)) return [];
	const props = schema.properties;
	const required = Array.isArray(schema.required) ? schema.required : [];
	const hints = isObj(ui) ? ui : {};
	const order = Array.isArray(hints.order)
		? hints.order.filter((n): n is string => typeof n === 'string' && n in props)
		: [];
	// Each field once, even if `order` repeats a name.
	const names = [...new Set([...order, ...Object.keys(props)])];
	return names.map((name) => {
		const field = isObj(props[name]) ? props[name] : {};
		const hint = isObj(hints[name]) ? hints[name] : {};
		const items = isObj(field.items) ? field.items : {};
		const choices = field.type === 'array' ? items.enum : field.enum;
		return {
			name,
			label: text(field.title) ?? name,
			kind: kindOf(field, hint),
			required: required.includes(name),
			help: text(hint.help) ?? text(field.description),
			placeholder: text(hint.placeholder),
			options: Array.isArray(choices) ? choices.map(String) : [],
			showIf: showIfOf(hint.show_if),
			lookup: isObj(hint.options_from)
		};
	});
}

/** Answers ready to submit: empty optional fields left out, numbers typed. */
export function answersOf(
	fields: Field[],
	values: Record<string, unknown>
): Record<string, unknown> {
	const out: Record<string, unknown> = {};
	const shown = shownFields(fields, values);
	for (const field of fields) {
		if (!shown.has(field.name)) continue;
		const value = values[field.name];
		if (value === undefined || value === '' || value === null) continue;
		if (field.kind === 'number' || field.kind === 'integer') {
			const n = Number(value);
			out[field.name] = Number.isNaN(n) ? value : n;
		} else if (field.kind === 'multiselect') {
			if (Array.isArray(value) && value.length) out[field.name] = value;
		} else {
			out[field.name] = value;
		}
	}
	return out;
}
