/**
 * Forms (design doc §6.8) for packs: a JSON Schema of answers with UI
 * hints, as `forms/<key>.json`. Workflows start from them with a `form`
 * trigger.
 */

type FieldOptions = {
	label: string;
	help?: string;
	placeholder?: string;
	required?: boolean;
	/** Shown only when another field has one of these answers. */
	showIf?: { field: string; equals: unknown[] };
};

interface FieldDef {
	schema: Record<string, unknown>;
	ui: Record<string, unknown>;
	required: boolean;
}

function fieldOf(
	schema: Record<string, unknown>,
	o: FieldOptions,
	ui: Record<string, unknown> = {}
): FieldDef {
	const hints: Record<string, unknown> = { ...ui };
	if (o.help) hints.help = o.help;
	if (o.placeholder) hints.placeholder = o.placeholder;
	if (o.showIf) hints.show_if = { field: o.showIf.field, equals: o.showIf.equals };
	return { schema: { title: o.label, ...schema }, ui: hints, required: o.required ?? false };
}

/** Form fields. */
export const field = {
	text: (o: FieldOptions & { maxLength?: number; multiline?: boolean }) =>
		fieldOf(
			{ type: 'string', ...(o.maxLength ? { maxLength: o.maxLength } : {}) },
			o,
			o.multiline ? { widget: 'textarea' } : {}
		),
	email: (o: FieldOptions) => fieldOf({ type: 'string', format: 'email' }, o),
	date: (o: FieldOptions) => fieldOf({ type: 'string', format: 'date' }, o),
	choice: (choices: string[], o: FieldOptions) => fieldOf({ type: 'string', enum: choices }, o),
	choices: (choices: string[], o: FieldOptions) =>
		fieldOf({ type: 'array', items: { type: 'string', enum: choices } }, o),
	number: (o: FieldOptions & { integer?: boolean }) =>
		fieldOf({ type: o.integer ? 'integer' : 'number' }, o),
	yesNo: (o: FieldOptions) => fieldOf({ type: 'boolean' }, o)
};

export interface FormDefinition {
	key: string;
	name: string;
	description?: string;
	fields: Record<string, FieldDef>;
	/** Offer it as an Action on a client, contact or ticket. */
	actionFor?: 'client' | 'contact' | 'ticket';
	/** Offer it in the client portal. */
	portal?: boolean;
	portalCategory?: string;
}

/** A pack form file's content (its key is the file name). */
export interface FormDocument {
	key: string;
	name: string;
	description?: string;
	schema: Record<string, unknown>;
	ui: Record<string, unknown>;
	actionFor?: string;
	portal?: boolean;
	portalCategory?: string;
}

export function form(def: FormDefinition): FormDocument {
	const entries = Object.entries(def.fields);
	const required = entries.filter(([, f]) => f.required).map(([k]) => k);
	const ui: Record<string, unknown> = { order: entries.map(([k]) => k) };
	for (const [k, f] of entries) if (Object.keys(f.ui).length > 0) ui[k] = f.ui;
	const doc: FormDocument = {
		key: def.key,
		name: def.name,
		schema: {
			type: 'object',
			properties: Object.fromEntries(entries.map(([k, f]) => [k, f.schema])),
			...(required.length > 0 ? { required } : {})
		},
		ui
	};
	if (def.description) doc.description = def.description;
	if (def.actionFor) doc.actionFor = def.actionFor;
	if (def.portal) doc.portal = true;
	if (def.portalCategory) doc.portalCategory = def.portalCategory;
	return doc;
}
