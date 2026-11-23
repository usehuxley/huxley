import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** A form field's looked-up choices, for the organization it's filled in for. */
export const GET: RequestHandler = async (event) => {
	const { id = '', name = '' } = event.params;
	const orgId = event.url.searchParams.get('orgId') ?? undefined;
	const { data, response } = await api(event).GET('/api/v1/forms/{id}/fields/{name}/options', {
		params: { path: { id, name }, query: { orgId } }
	});
	if (!data) return json([], { status: response.status });
	return json(data, { headers: { 'cache-control': 'private, no-store' } });
};
