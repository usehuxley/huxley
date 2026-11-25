import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { articleFields } from '#lib/kb/form.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { user, currentOrg } = await event.parent();
	if (!user.grantedPermissions.includes('kb:write')) error(403, 'You may not write articles.');
	const client = api(event);
	const [orgs, categories] = await Promise.all([
		client.GET('/api/v1/orgs'),
		client.GET('/api/v1/kb/categories')
	]);
	if (categories.response.status === 404) error(404, 'The knowledge base is turned off.');
	const list = orgs.data ?? [];
	// The current organization first, then the root (the MSP), then the rest.
	// ?org= (e.g. from a client's Documentation), else the current organization, else the root.
	const first =
		event.url.searchParams.get('org') ??
		currentOrg?.id ??
		list.find((o) => o.parentId === null)?.id;
	return {
		orgs: [...list.filter((o) => o.id === first), ...list.filter((o) => o.id !== first)].map(
			(o) => ({
				value: o.id,
				label: o.name
			})
		),
		categories: (categories.data ?? []).map((c) => ({ value: c.id, label: c.name }))
	};
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/kb/articles', {
			body: { orgId: formString(form, 'orgId'), ...articleFields(form) }
		});
		if (!data) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not save the article.')
			});
		}
		redirect(303, `/knowledge/${data.id}`);
	}
};
