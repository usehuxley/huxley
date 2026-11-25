import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** Articles to link from a ticket: the best matches for `q`. */
export const GET: RequestHandler = async (event) => {
	const q = event.url.searchParams.get('q')?.trim() ?? '';
	if (q.length < 2) return json([]);
	const { data } = await api(event).GET('/api/v1/kb/articles', {
		params: { query: { q, limit: 8 } }
	});
	const found = (data?.items ?? [])
		.filter((a) => a.status !== 'archived')
		.map((a) => ({ id: a.id, title: a.title, status: a.status, orgName: a.orgName }));
	return json(found, { headers: { 'cache-control': 'private, no-store' } });
};
