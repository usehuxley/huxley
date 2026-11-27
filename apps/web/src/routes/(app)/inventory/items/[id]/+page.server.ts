import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { serialList } from '#lib/inventory/labels.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type MovementKind = components['schemas']['InventoryMovementKind'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [detail, movements, units, locations] = await Promise.all([
		client.GET('/api/v1/inventory/items/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/inventory/movements', { params: { query: { itemId: id, limit: 50 } } }),
		client.GET('/api/v1/inventory/units', { params: { query: { itemId: id } } }),
		client.GET('/api/v1/inventory/locations')
	]);
	if (detail.response.status === 404) error(404, 'Item not found.');
	if (!detail.data) error(503, 'Could not load the item.');
	const { user } = await event.parent();
	const item = detail.data.item;
	return {
		item,
		levels: detail.data.levels,
		movements: movements.data ?? [],
		units: units.data ?? [],
		locations: (locations.data ?? [])
			.filter((l) => l.orgId === item.orgId)
			.map((l) => ({ value: l.id, label: l.parentName ? `${l.parentName} › ${l.name}` : l.name })),
		canWrite: user.grantedPermissions.includes('inventory:write'),
		canConfigure: user.grantedPermissions.includes('inventory:configure')
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

export const actions: Actions = {
	move: async (event) => {
		const form = await event.request.formData();
		const kind = formString(form, 'kind') as MovementKind;
		const quantity = formString(form, 'quantity').trim();
		const unitCost = formString(form, 'unitCost').trim();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/inventory/movements', {
			body: {
				kind,
				itemId: event.params.id,
				locationId: formString(form, 'locationId'),
				toLocationId: formString(form, 'toLocationId') || null,
				quantity: quantity || null,
				lot: formString(form, 'lot') || null,
				serials: serialList(formString(form, 'serials')),
				unitCost: unitCost || null,
				reference: formString(form, 'reference') || null,
				note: formString(form, 'note') || null
			}
		});
		if (!data) return failed(response.status, problem, 'Could not record it.');
		return { moved: kind };
	},
	reorder: async (event) => {
		const form = await event.request.formData();
		const location = formString(form, 'locationId');
		const point = formString(form, 'reorderPoint').trim();
		const client = api(event);
		const path = { id: event.params.id, location };
		const { response, error: problem } = point
			? await client.PUT('/api/v1/inventory/items/{id}/reorder/{location}', {
					params: { path },
					body: { reorderPoint: point, reorderQty: formString(form, 'reorderQty') || null }
				})
			: await client.DELETE('/api/v1/inventory/items/{id}/reorder/{location}', {
					params: { path }
				});
		if (!response.ok) return failed(response.status, problem, 'Could not set the reorder point.');
		return { reorder: true };
	}
};
