import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const range = event.url.searchParams.get('range') ?? undefined;
	const { data } = await api(event).GET('/api/portal/v1/dashboards/{id}', {
		params: { path: { id: event.params.id }, query: { range } }
	});
	if (!data) error(404, 'There’s no such report.');
	return { view: data };
};
