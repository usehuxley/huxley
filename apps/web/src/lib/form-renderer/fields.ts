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

export interface Field {
	name: string;
	label: string;
	kind: FieldKind;
	required: boolean;
	help?: string;
	placeholder?: string;
	options: string[];
}

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v);
const text = (v: unknown) => (typeof v === 'string' ? v : undefined);

function kindOf(schema: Obj, hint: Obj): FieldKind {
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
			options: Array.isArray(choices) ? choices.map(String) : []
		};
	});
}

/** Answers ready to submit: empty optional fields left out, numbers typed. */
export function answersOf(
	fields: Field[],
	values: Record<string, unknown>
): Record<string, unknown> {
	const out: Record<string, unknown> = {};
	for (const field of fields) {
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
