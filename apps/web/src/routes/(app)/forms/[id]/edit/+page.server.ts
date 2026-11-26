import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const params = { path: { id: event.params.id } };
	const [found, submissions, links] = await Promise.all([
		client.GET('/api/v1/forms/{id}', { params }),
		client.GET('/api/v1/forms/{id}/submissions', { params }),
		client.GET('/api/v1/forms/{id}/links', { params })
	]);
	if (found.response.status === 404) error(404, 'Form not found.');
	if (!found.data) error(503, 'Could not load the form.');
	return {
		form: found.data,
		submissions: submissions.data ?? [],
		links: links.data ?? [],
		origin: event.url.origin
	};
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
				actionFor: formString(form, 'actionFor') || null,
				portal: form.get('portal') === 'on',
				portalCategory: formString(form, 'portalCategory') || null,
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
	link: async (event) => {
		const form = await event.request.formData();
		const expires = formString(form, 'expiresAt');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/forms/{id}/links', {
			params: { path: { id: event.params.id } },
			body: {
				name: formString(form, 'name'),
				expiresAt: expires ? new Date(expires).toISOString() : null
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not make the link.')
			});
		}
		return { linkToken: data.token };
	},
	revokeLink: async (event) => {
		const linkId = formString(await event.request.formData(), 'linkId');
		await api(event).POST('/api/v1/forms/{id}/links/{link_id}/revoke', {
			params: { path: { id: event.params.id, link_id: linkId } }
		});
		return { message: 'The link no longer works.' };
	},
	restore: async (event) => {
		await api(event).POST('/api/v1/forms/{id}/restore', {
			params: { path: { id: event.params.id } }
		});
		return { message: 'Restored.' };
	}
};
