import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';

const NO_STORE = { 'cache-control': 'no-store, private' };

/** Reveals a vault secret for the page (logged and audited by the API); never cached. */
export const POST: RequestHandler = async (event) => {
	const { reason } = (await event.request.json().catch(() => ({}))) as { reason?: unknown };
	const { data, error, response } = await api(event).POST('/api/v1/vault/items/{id}/reveal', {
		params: { path: { id: event.params.id ?? '' } },
		body: { reason: typeof reason === 'string' && reason.trim() ? reason.trim() : null }
	});
	if (!data) {
		return json(
			{ message: problemMessage(error, 'Could not reveal it.') },
			{ status: response.status, headers: NO_STORE }
		);
	}
	return json({ secret: data.secret }, { headers: NO_STORE });
};
