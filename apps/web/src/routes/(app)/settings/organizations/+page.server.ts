import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/orgs');
	if (response.status === 403) error(403, 'You do not have access to organizations.');
	if (!data) error(503, 'Could not load organizations.');
	return { orgs: data };
};

const version = (form: FormData) => Number(formString(form, 'version'));

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const { error: problem } = await api(event).POST('/api/v1/orgs', {
			body: {
				parentId: formString(form, 'parentId'),
				kind: formString(form, 'kind') === 'provider' ? 'provider' : 'client',
				name: formString(form, 'name')
			}
		});
		if (problem) return fail(400, { message: problemMessage(problem, 'Could not create it.') });
		return { message: 'Organization created.' };
	},

	rename: async (event) => {
		const form = await event.request.formData();
		const { error: problem } = await api(event).PATCH('/api/v1/orgs/{id}', {
			params: { path: { id: formString(form, 'id') } },
			body: { name: formString(form, 'name'), version: version(form) }
		});
		if (problem) return fail(400, { message: problemMessage(problem, 'Could not rename it.') });
		return { message: 'Organization renamed.' };
	},

	move: async (event) => {
		const form = await event.request.formData();
		const { error: problem } = await api(event).POST('/api/v1/orgs/{id}/move', {
			params: { path: { id: formString(form, 'id') } },
			body: { parentId: formString(form, 'parentId'), version: version(form) }
		});
		if (problem) return fail(400, { message: problemMessage(problem, 'Could not move it.') });
		return { message: 'Organization moved.' };
	},

	archive: async (event) => {
		const form = await event.request.formData();
		const { error: problem } = await api(event).POST('/api/v1/orgs/{id}/archive', {
			params: { path: { id: formString(form, 'id') } },
			body: { version: version(form) }
		});
		if (problem) return fail(400, { message: problemMessage(problem, 'Could not archive it.') });
		return { message: 'Organization archived.' };
	}
};
