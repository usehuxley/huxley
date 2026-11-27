import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

type Tracking = 'none' | 'serial' | 'lot';

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const client = api(event);
	const filter = {
		q: params.get('q') || undefined,
		low: params.get('low') === '1',
		locationId: params.get('location') || undefined
	};
	const { user } = await event.parent();
	const canConfigure = user.grantedPermissions.includes('inventory:configure');
	const [items, locations, products, orgs] = await Promise.all([
		client.GET('/api/v1/inventory/items', { params: { query: filter } }),
		client.GET('/api/v1/inventory/locations'),
		canConfigure
			? client.GET('/api/v1/catalog/products', { params: { query: { active: true } } })
			: null,
		canConfigure ? client.GET('/api/v1/orgs') : null
	]);
	if (items.response.status === 404) error(404, 'Inventory is turned off.');
	if (!items.data) error(items.response.status === 403 ? 403 : 503, 'Could not load inventory.');
	return {
		items: items.data,
		locations: (locations.data ?? []).map((l) => ({ value: l.id, label: l.name })),
		products: (products?.data ?? [])
			.filter((p) => p.kind === 'product')
			.map((p) => ({ value: p.id, label: `${p.name} · ${p.sku}` })),
		providers: (orgs?.data ?? [])
			.filter((o) => o.kind === 'provider')
			.map((o) => ({ value: o.id, label: o.name })),
		q: params.get('q') ?? '',
		low: filter.low,
		location: params.get('location') ?? '',
		canConfigure
	};
};

export const actions: Actions = {
	stock: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/inventory/items', {
			body: {
				orgId: formString(form, 'orgId'),
				item: {
					productId: formString(form, 'productId'),
					tracking: (formString(form, 'tracking') || 'none') as Tracking
				}
			}
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not stock it.') });
		}
		redirect(303, `/inventory/items/${data.id}`);
	}
};
