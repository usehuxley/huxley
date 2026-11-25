import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** Checks in on a ticket (`{ typing }` or `{ leaving }`): who else is on it. */
export const POST: RequestHandler = async (event) => {
	const body: unknown = await event.request.json().catch(() => ({}));
	const read = (key: string) =>
		typeof body === 'object' &&
		body !== null &&
		key in body &&
		(body as Record<string, unknown>)[key] === true;
	const { data } = await api(event).POST('/api/v1/tickets/{id}/presence', {
		params: { path: { id: event.params.id ?? '' } },
		body: { typing: read('typing'), leaving: read('leaving') }
	});
	return json(data ?? [], { headers: { 'cache-control': 'private, no-store' } });
};
