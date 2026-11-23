import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { keyFromName } from '#lib/workflows/yaml.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [templates, orgs] = await Promise.all([
		client.GET('/api/v1/templates'),
		client.GET('/api/v1/orgs')
	]);
	const template = templates.data?.find((t) => t.key === event.params.key);
	if (!template) error(404, 'No such template.');
	return { template, orgs: orgs.data ?? [] };
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const name = formString(form, 'name').trim();
		const key = formString(form, 'key').trim() || keyFromName(name);
		const orgId = formString(form, 'orgId');
		if (!name || !orgId) {
			return fail(400, { message: 'Give it a name and an organization.', errors: {} });
		}
		const bindings: Record<string, string> = {};
		for (const [k, v] of form.entries()) {
			if (k.startsWith('binding.') && typeof v === 'string') bindings[k.slice(8)] = v;
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/templates/{key}/use', {
			params: { path: { key: event.params.key } },
			body: { orgId, key, name, bindings }
		});
		if (!data) {
			const errors: Record<string, string> = {};
			const list = (problem as { errors?: { pointer?: string; message?: string }[] } | undefined)
				?.errors;
			for (const e of list ?? []) {
				if (e.pointer?.startsWith('bindings.') && e.message) errors[e.pointer.slice(9)] = e.message;
			}
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create it.'),
				errors
			});
		}
		redirect(303, `/workflows/${data.workflowId}`);
	}
};
