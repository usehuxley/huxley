import type { components } from '@usehuxley/api-client';

type FieldValue = components['schemas']['FieldValue'];

/**
 * Reads the inputs FieldInput.svelte renders (named `f:<key>`) back into
 * typed values for the API. Empty inputs become null (clear the value).
 */
export function readFieldValues(form: FormData, fields: FieldValue[]): Record<string, unknown> {
	const values: Record<string, unknown> = {};
	for (const field of fields) {
		const name = `f:${field.key}`;
		const raw = form.get(name);
		const text = typeof raw === 'string' ? raw.trim() : '';
		switch (field.fieldType) {
			case 'boolean':
				values[field.key] = form.get(name) === 'on';
				break;
			case 'number':
				values[field.key] = text === '' ? null : Number(text);
				break;
			case 'multi_select': {
				const picked = form.getAll(name).filter((v): v is string => typeof v === 'string');
				values[field.key] = picked.length ? picked : null;
				break;
			}
			default:
				values[field.key] = text === '' ? null : text;
		}
	}
	return values;
}
