import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { keyFromName, starter } from '#lib/workflows/yaml.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [workflows, orgs] = await Promise.all([
		client.GET('/api/v1/workflows'),
		client.GET('/api/v1/orgs')
	]);
	if (!workflows.data) error(503, 'Could not load workflows.');
	return { workflows: workflows.data, orgs: orgs.data ?? [] };
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const name = formString(form, 'name').trim();
		const key = formString(form, 'key').trim() || keyFromName(name);
		const orgId = formString(form, 'orgId');
		if (!name || !orgId)
			return fail(400, { message: 'Give the workflow a name and an organization.' });
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/workflows', {
			body: { orgId, document: starter(key, name) }
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create the workflow.')
			});
		}
		redirect(303, `/workflows/${data.id}`);
	}
};
