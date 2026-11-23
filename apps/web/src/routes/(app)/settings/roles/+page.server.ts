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
	if (roles.response.status === 403) error(403, 'You cannot view roles.');
	if (!roles.data || !permissions.data) error(503, 'Could not load roles.');
	return { roles: roles.data, permissions: permissions.data };
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const permissions = form
			.getAll('permissions')
			.filter((p): p is string => typeof p === 'string');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/roles', {
			body: {
				name: formString(form, 'name'),
				description: formString(form, 'description'),
				permissions
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create the role.')
			});
		}
		redirect(303, `/settings/roles/${data.id}`);
	}
};
