import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type Kind = components['schemas']['InventoryLocationKind'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [locations, orgs, users] = await Promise.all([
		client.GET('/api/v1/inventory/locations', { params: { query: { all: true } } }),
		client.GET('/api/v1/orgs'),
		client.GET('/api/v1/users', { params: { query: { limit: 200 } } })
	]);
	if (locations.response.status === 404) error(404, 'Inventory is turned off.');
	if (!locations.data) error(503, 'Could not load locations.');
	return {
		locations: locations.data,
		providers: (orgs.data ?? [])
			.filter((o) => o.kind === 'provider')
			.map((o) => ({ value: o.id, label: o.name })),
		people: (users.data?.items ?? [])
			.filter((u) => u.status === 'active' && u.kind === 'human')
			.map((u) => ({ value: u.id, label: u.displayName }))
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const location = {
			name: formString(form, 'name'),
			kind: (formString(form, 'kind') || 'warehouse') as Kind,
			parentId: formString(form, 'parentId') || null,
			assignedUserId: formString(form, 'assignedUserId') || null,
			address: formString(form, 'address') || null,
			active: form.get('active') === 'on'
		};
		const client = api(event);
		const {
			data,
			error: problem,
			response
		} = id
			? await client.PUT('/api/v1/inventory/locations/{id}', {
					params: { path: { id } },
					body: location
				})
			: await client.POST('/api/v1/inventory/locations', {
					body: { orgId: formString(form, 'orgId'), location }
				});
		if (!data) return failed(response.status, problem, 'Could not save the location.');
		return { saved: data.name };
	},
	delete: async (event) => {
		const form = await event.request.formData();
		const { response, error: problem } = await api(event).DELETE(
			'/api/v1/inventory/locations/{id}',
			{ params: { path: { id: formString(form, 'id') } } }
		);
		if (!response.ok) return failed(response.status, problem, 'Could not remove it.');
		return { removed: true };
	}
};
