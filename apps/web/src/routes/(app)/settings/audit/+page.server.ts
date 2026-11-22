import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const cursor = event.url.searchParams.get('cursor') ?? undefined;
	const action = event.url.searchParams.get('action')?.trim() || undefined;
	const { data, response } = await api(event).GET('/api/v1/audit', {
		params: { query: { cursor, action, limit: 50 } }
	});
	if (response.status === 403) error(403, 'You do not have access to the audit log.');
	if (!data) error(503, 'Could not load the audit log.');
	return { page: data, action: action ?? '' };
};
