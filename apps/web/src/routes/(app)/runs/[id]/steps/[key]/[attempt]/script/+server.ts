import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** A script step attempt's task: where it ran, how it ended, its logs. */
export const GET: RequestHandler = async (event) => {
	const { id = '', key = '', attempt = '' } = event.params;
	const { data, response } = await api(event).GET(
		'/api/v1/runs/{id}/steps/{key}/{attempt}/script',
		{
			params: { path: { id, key, attempt: Number(attempt) } }
		}
	);
	if (!data) return json(null, { status: response.status === 403 ? 403 : 404 });
	return json(data, { headers: { 'cache-control': 'private, no-store' } });
};
