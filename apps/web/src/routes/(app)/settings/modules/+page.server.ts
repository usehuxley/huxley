import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/v1/modules');
	if (!data) error(503, 'Could not load modules.');
	return { modules: data };
};

export const actions: Actions = {
	toggle: async (event) => {
		const form = await event.request.formData();
		const key = formString(form, 'key');
		const enabled = formString(form, 'enabled') === 'true';
		const { data, error: problem } = await api(event).PUT('/api/v1/modules/{key}', {
			params: { path: { key } },
			body: { enabled, includeDependencies: true }
		});
		if (!data)
			return fail(400, { message: problemMessage(problem, 'Could not change the module.') });
		return { changed: data.changed, enabled };
	}
};
