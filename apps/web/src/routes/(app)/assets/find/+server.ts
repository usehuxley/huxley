import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** Assets to link from a ticket: in use, matching `q`, of `org` if given. */
export const GET: RequestHandler = async (event) => {
	const q = event.url.searchParams.get('q')?.trim() ?? '';
	if (q.length < 2) return json([]);
	const orgId = event.url.searchParams.get('org') || undefined;
	const { data } = await api(event).GET('/api/v1/assets', {
		params: { query: { q, orgId, inUse: true, limit: 8 } }
	});
	const found = (data?.items ?? []).map((a) => ({ id: a.id, name: a.name, typeName: a.typeName }));
	return json(found, { headers: { 'cache-control': 'private, no-store' } });
};
