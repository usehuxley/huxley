import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { user, currentOrg } = await event.parent();
	if (!user.grantedPermissions.includes('kb:publish')) error(403, 'You may not keep categories.');
	const client = api(event);
	const [categories, orgs] = await Promise.all([
		client.GET('/api/v1/kb/categories'),
		client.GET('/api/v1/orgs')
	]);
	if (categories.response.status === 404) error(404, 'The knowledge base is turned off.');
	if (!categories.data) error(503, 'Could not load the categories.');
	const list = orgs.data ?? [];
	const names = Object.fromEntries(list.map((o) => [o.id, o.name]));
	const first = currentOrg?.id ?? list.find((o) => o.parentId === null)?.id;
	return {
		categories: categories.data.map((c) => ({ ...c, orgName: names[c.orgId] ?? '' })),
		orgs: [...list.filter((o) => o.id === first), ...list.filter((o) => o.id !== first)].map(
			(o) => ({
				value: o.id,
				label: o.name
			})
		)
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { errors: fieldErrors(problem), message: problemMessage(problem, fallback) });

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const category = {
			name: formString(form, 'name'),
			description: formString(form, 'description') || null,
			position: Number(formString(form, 'position')) || 0
		};
		const { error: problem, response } = id
			? await api(event).PUT('/api/v1/kb/categories/{id}', {
					params: { path: { id } },
					body: category
				})
			: await api(event).POST('/api/v1/kb/categories', {
					body: { orgId: formString(form, 'orgId'), category }
				});
		if (response.status !== 200 && response.status !== 201)
			return failed(response.status, problem, 'Could not save the category.');
		return { saved: true };
	},
	delete: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/kb/categories/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) return failed(response.status, problem, 'Could not delete it.');
		return { saved: true };
	}
};
