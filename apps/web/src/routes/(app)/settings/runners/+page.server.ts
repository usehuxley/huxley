import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/runners');
	if (response.status === 403) error(403, 'You do not have access to script runners.');
	if (!data) error(503, 'Could not load runners.');
	return { runners: data, server: event.url.origin };
};

export const actions: Actions = {
	enroll: async (event) => {
		const name = formString(await event.request.formData(), 'name').trim();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/runners/enrollments', { body: { name } });
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not create it.') });
		}
		return { token: data.token, name, expiresAt: data.expiresAt };
	},
	external: async (event) => {
		const name = formString(await event.request.formData(), 'name').trim();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/runners/external', {
			body: { name }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not register it.') });
		}
		return { message: `Registered ${data.name}.` };
	},
	revoke: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/runners/{id}/revoke', { params: { path: { id } } });
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not revoke it.') });
		}
		return { message: `Revoked ${data.name}.` };
	}
};
