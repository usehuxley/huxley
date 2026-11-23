import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const params = { path: { id: event.params.id } };
	const [found, submissions] = await Promise.all([
		client.GET('/api/v1/forms/{id}', { params }),
		client.GET('/api/v1/forms/{id}/submissions', { params })
	]);
	if (found.response.status === 404) error(404, 'Form not found.');
	if (!found.data) error(503, 'Could not load the form.');
	return { form: found.data, submissions: submissions.data ?? [] };
};

function parse(text: string): unknown {
	return text.trim() ? JSON.parse(text) : {};
}

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		let schema: unknown;
		let ui: unknown;
		try {
			schema = parse(formString(form, 'schema'));
			ui = parse(formString(form, 'ui'));
		} catch {
			return fail(400, { message: 'The schema and UI hints must be valid JSON.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/forms/{id}', {
			params: { path: { id: event.params.id } },
			body: {
				key: formString(form, 'key'),
				name: formString(form, 'name'),
				description: formString(form, 'description') || null,
				schema: schema as Record<string, unknown>,
				ui: ui as Record<string, unknown>
			}
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not save it.') });
		}
		return { message: 'Saved.' };
	},
	archive: async (event) => {
		await api(event).POST('/api/v1/forms/{id}/archive', {
			params: { path: { id: event.params.id } }
		});
		return { message: 'Archived. It can no longer be submitted.' };
	},
	restore: async (event) => {
		await api(event).POST('/api/v1/forms/{id}/restore', {
			params: { path: { id: event.params.id } }
		});
		return { message: 'Restored.' };
	}
};
