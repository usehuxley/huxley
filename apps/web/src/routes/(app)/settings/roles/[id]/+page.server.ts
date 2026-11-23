import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [roles, permissions] = await Promise.all([
		client.GET('/api/v1/roles'),
		client.GET('/api/v1/permissions')
	]);
	const role = roles.data?.find((r) => r.id === event.params.id);
	if (!role) error(404, 'Role not found.');
	return { role, permissions: permissions.data ?? [] };
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const permissions = form
			.getAll('permissions')
			.filter((p): p is string => typeof p === 'string');
		const { error: problem, response } = await api(event).PUT('/api/v1/roles/{id}', {
			params: { path: { id: event.params.id } },
			body: {
				name: formString(form, 'name'),
				description: formString(form, 'description'),
				permissions
			}
		});
		if (response.status !== 200) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not save the role.')
			});
		}
		return { saved: true };
	},

	delete: async (event) => {
		const { error: problem, response } = await api(event).DELETE('/api/v1/roles/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not delete the role.')
			});
		}
		redirect(303, '/settings/roles');
	}
};
