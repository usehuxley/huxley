import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { specFrom } from '#lib/integrations/spec.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [integrations, orgs, catalog] = await Promise.all([
		client.GET('/api/v1/integrations'),
		client.GET('/api/v1/orgs'),
		client.GET('/api/v1/integrations/catalog')
	]);
	if (!integrations.data) error(503, 'Could not load integrations.');
	return { integrations: integrations.data, orgs: orgs.data ?? [], catalog: catalog.data ?? [] };
};

export const actions: Actions = {
	addBuiltIn: async (event) => {
		const form = await event.request.formData();
		const orgId = formString(form, 'orgId');
		const key = formString(form, 'key');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/integrations/catalog/{key}', {
			params: { path: { key } },
			body: { orgId }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not add it.') });
		}
		redirect(303, `/integrations/${data.id}`);
	},
	import: async (event) => {
		const form = await event.request.formData();
		const orgId = formString(form, 'orgId');
		const key = formString(form, 'key').trim();
		const spec = await specFrom(form);
		if (!orgId || !spec.trim()) {
			return fail(400, { message: 'Choose an organization and give a spec (paste or upload).' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/integrations', {
			body: { orgId, key: key || null, spec }
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not import the spec.')
			});
		}
		redirect(303, `/integrations/${data.id}`);
	}
};
