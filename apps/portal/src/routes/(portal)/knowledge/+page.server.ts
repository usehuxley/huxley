import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const q = event.url.searchParams.get('q')?.trim() ?? '';
	const category = event.url.searchParams.get('category') ?? '';
	const { data, response } = await api(event).GET('/api/portal/v1/articles', {
		params: { query: { q: q || undefined, category: category || undefined } }
	});
	if (!data) error(response.status === 403 ? 404 : 503, 'Help articles aren’t available.');
	return { ...data, q, category };
};
