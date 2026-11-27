import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const status = (['open', 'fulfilled', 'closed', 'cancelled'] as const).find(
		(s) => s === params.get('status')
	);
	const { data, response } = await api(event).GET('/api/v1/sales-orders', {
		params: { query: { status, q: params.get('q') || undefined } }
	});
	if (response.status === 404) error(404, 'Sales is turned off.');
	if (!data) error(503, 'Could not load orders.');
	return { orders: data, status: status ?? '', q: params.get('q') ?? '' };
};
