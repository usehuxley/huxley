import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const q = event.url.searchParams.get('q')?.trim() ?? '';
	const { data, response } = await api(event).GET('/api/portal/v1/devices', {
		params: { query: { q: q || undefined } }
	});
	if (!data) error(response.status === 403 ? 404 : 503, 'Devices aren’t available.');
	return { devices: data, q };
};
