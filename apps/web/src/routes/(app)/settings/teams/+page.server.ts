import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/teams');
	if (response.status === 403) error(403, 'You do not have access to teams.');
	if (!data) error(503, 'Could not load teams.');
	return { teams: data };
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/teams', {
			body: { name: formString(form, 'name'), description: formString(form, 'description') }
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create the team.')
			});
		}
		redirect(303, `/settings/teams/${data.id}`);
	}
};
