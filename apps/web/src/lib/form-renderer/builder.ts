/**
 * The form builder's model: a list of fields that converts to and from a
 * form's JSON Schema and UI hints. Schema details the builder doesn't edit
 * (e.g. `minLength`, `pattern`) are kept as they were.
 */
import { fieldsOf, type FieldKind } from './fields.ts';

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v);

export interface BuilderField {
	name: string;
	label: string;
	kind: FieldKind;
	required: boolean;
	help: string;
	placeholder: string;
	options: string[];
	/** The rest of the field's schema, kept as it was. */
	extra: Obj;
}

export const KINDS: { value: FieldKind; label: string }[] = [
	{ value: 'text', label: 'Text' },
	{ value: 'textarea', label: 'Long text' },
	{ value: 'email', label: 'Email' },
	{ value: 'number', label: 'Number' },
	{ value: 'integer', label: 'Whole number' },
	{ value: 'date', label: 'Date' },
	{ value: 'datetime', label: 'Date and time' },
	{ value: 'boolean', label: 'Checkbox' },
	{ value: 'select', label: 'Choice' },
	{ value: 'multiselect', label: 'Multiple choice' }
];

/** Keys the builder sets; everything else in a field's schema is kept. */
const MANAGED = ['type', 'format', 'enum', 'items', 'title', 'uniqueItems'];

export const hasOptions = (kind: FieldKind) => kind === 'select' || kind === 'multiselect';

/** The builder's fields for a form's schema and UI hints. */
export function builderFieldsOf(schema: unknown, ui: unknown): BuilderField[] {
	const props = isObj(schema) && isObj(schema.properties) ? schema.properties : {};
	const hintsOf: Obj = isObj(ui) ? ui : {};
	return fieldsOf(schema, ui).map((f) => {
		const ownValue = props[f.name];
		const own: Obj = isObj(ownValue) ? ownValue : {};
		const extra = Object.fromEntries(Object.entries(own).filter(([k]) => !MANAGED.includes(k)));
		const hintValue = hintsOf[f.name];
		const hints: Obj = isObj(hintValue) ? hintValue : {};
		// Help shown from `description` stays there unless a hint overrides it.
		const help = typeof hints.help === 'string' ? hints.help : '';
		return {
			name: f.name,
			label: f.label === f.name && typeof own.title !== 'string' ? '' : f.label,
			kind: f.kind,
			required: f.required,
			help,
			placeholder: f.placeholder ?? '',
			options: f.options,
			extra
		};
	});
}

function propertyOf(field: BuilderField): Obj {
	const out: Obj = Object.fromEntries(
		Object.entries(field.extra).filter(([k]) => !MANAGED.includes(k))
	);
	if (field.label.trim()) out.title = field.label.trim();
	const options = field.options.map((o) => o.trim()).filter(Boolean);
	switch (field.kind) {
		case 'boolean':
		case 'number':
		case 'integer':
			out.type = field.kind;
			break;
		case 'email':
			Object.assign(out, { type: 'string', format: 'email' });
			break;
		case 'date':
			Object.assign(out, { type: 'string', format: 'date' });
			break;
		case 'datetime':
			Object.assign(out, { type: 'string', format: 'date-time' });
			break;
		case 'select':
			Object.assign(out, { type: 'string', enum: options });
			break;
		case 'multiselect':
			Object.assign(out, {
				type: 'array',
				items: { type: 'string', enum: options },
				uniqueItems: true
			});
			break;
		default:
			out.type = 'string';
	}
	return out;
}

/** The JSON Schema and UI hints for the builder's fields. */
export function definitionOf(
	fields: BuilderField[],
	previousUi: unknown = {}
): { schema: Obj; ui: Obj } {
	const properties: Obj = {};
	const required: string[] = [];
	// Hints for fields the builder doesn't know about are kept.
	const ui: Obj = {};
	if (isObj(previousUi)) {
		for (const [k, v] of Object.entries(previousUi)) {
			if (k !== 'order' && !fields.some((f) => f.name === k)) ui[k] = v;
		}
	}
	for (const field of fields) {
		properties[field.name] = propertyOf(field);
		if (field.required) required.push(field.name);
		const hint: Obj = {};
		if (field.kind === 'textarea') hint.widget = 'textarea';
		if (field.help.trim()) hint.help = field.help.trim();
		if (field.placeholder.trim()) hint.placeholder = field.placeholder.trim();
		if (Object.keys(hint).length) ui[field.name] = hint;
		else delete ui[field.name];
	}
	ui.order = [...new Set(fields.map((f) => f.name))];
	const schema: Obj = { type: 'object', properties };
	if (required.length) schema.required = required;
	return { schema, ui };
}

/** A field name made from a label: `Start date` → `start_date`. */
export function nameFrom(label: string, taken: string[]): string {
	const base =
		label
			.toLowerCase()
			.normalize('NFKD')
			.replace(/[^a-z0-9]+/g, '_')
			.replace(/^_+|_+$/g, '')
			.replace(/^(\d)/, 'f_$1') || 'field';
	let name = base;
	for (let i = 2; taken.includes(name); i++) name = `${base}_${i}`;
	return name;
}

/** Problems that would stop the form saving, by field index. */
export function problemsOf(fields: BuilderField[]): Map<number, string> {
	const problems = new Map<number, string>();
	fields.forEach((f, i) => {
		if (!/^[a-z][a-z0-9_]{0,63}$/.test(f.name)) {
			problems.set(i, 'The name needs lowercase letters, digits and underscores.');
		} else if (fields.findIndex((g) => g.name === f.name) !== i) {
			problems.set(i, 'Another field has this name.');
		} else if (hasOptions(f.kind) && !f.options.some((o) => o.trim())) {
			problems.set(i, 'Add at least one choice.');
		}
	});
	return problems;
}
