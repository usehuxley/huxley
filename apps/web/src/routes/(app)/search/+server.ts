import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** Global search for the command palette: what the API finds for `q`. */
export const GET: RequestHandler = async (event) => {
	const q = event.url.searchParams.get('q')?.trim() ?? '';
	if (q.length < 2) return json([]);
	const { data } = await api(event).GET('/api/v1/search', { params: { query: { q } } });
	return json(data ?? [], { headers: { 'cache-control': 'private, no-store' } });
};
