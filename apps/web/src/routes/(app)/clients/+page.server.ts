import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const search = event.url.searchParams.get('q') ?? '';
	const { data, response } = await api(event).GET('/api/v1/companies', {
		params: { query: { search: search || undefined } }
	});
	if (response.status === 404) error(404, 'Clients and contacts are turned off.');
	if (!data) error(503, 'Could not load clients.');
	return { companies: data, search };
};
