import type { RequestEvent } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';

/** An opportunity's fields from a submitted form, for the API. */
export function opportunityBody(form: FormData) {
	const probability = formString(form, 'probability');
	return {
		name: formString(form, 'name'),
		description: formString(form, 'description'),
		amount: formString(form, 'amount') || '0',
		recurring: formString(form, 'recurring') || '0',
		currency: formString(form, 'currency') || 'USD',
		probability: probability ? Number(probability) : null,
		closeDate: formString(form, 'closeDate') || null,
		forecast: (formString(form, 'forecast') || 'pipeline') as
			'pipeline' | 'best_case' | 'commit' | 'omitted',
		ownerId: formString(form, 'ownerId') || null,
		contactId: formString(form, 'contactId') || null,
		source: formString(form, 'source') || null
	};
}

/** Field errors by name, from the API's `/field` pointers. */
export function fieldErrors(problem: unknown): Record<string, string> {
	const errors: Record<string, string> = {};
	const list = (problem as { errors?: { pointer?: string; message?: string }[] } | undefined)
		?.errors;
	for (const e of list ?? []) {
		const name = e.pointer?.split('/').filter(Boolean).pop();
		if (name && e.message) errors[name] = e.message;
	}
	return errors;
}

export type { RequestEvent };
