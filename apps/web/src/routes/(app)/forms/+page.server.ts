import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { keyFromName } from '#lib/workflows/yaml.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [forms, orgs] = await Promise.all([
		client.GET('/api/v1/forms'),
		client.GET('/api/v1/orgs')
	]);
	if (!forms.data) error(503, 'Could not load forms.');
	return { forms: forms.data, orgs: orgs.data ?? [] };
};

/** A form to start from: one required field. */
const STARTER = {
	type: 'object',
	required: ['name'],
	properties: { name: { type: 'string', title: 'Name', minLength: 1 } }
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const name = formString(form, 'name').trim();
		const orgId = formString(form, 'orgId');
		if (!name || !orgId) return fail(400, { message: 'Give the form a name and an organization.' });
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/forms', {
			body: { orgId, form: { key: keyFromName(name), name, schema: STARTER, ui: {} } }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not create it.') });
		}
		redirect(303, `/forms/${data.id}/edit`);
	}
};
