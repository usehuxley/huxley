import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/jobs/dead');
	if (response.status === 403) error(403, 'You do not have access to workflow jobs.');
	if (!data) error(503, 'Could not load workflow jobs.');
	return { jobs: data };
};

export const actions: Actions = {
	retry: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/jobs/{id}/retry', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not retry it.') });
		}
		return { message: data.retried ? 'Queued again.' : 'It was no longer set aside.' };
	}
};
