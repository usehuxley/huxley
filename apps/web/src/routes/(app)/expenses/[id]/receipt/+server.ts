import type { RequestHandler } from '@sveltejs/kit';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';
import { withheld } from '#lib/server/withheld.ts';

/** Streams an expense's receipt from the API. */
export const GET: RequestHandler = async (event) => {
	const id = encodeURIComponent(event.params.id ?? '');
	const upstream = await fetch(`${apiUrl}/api/v1/expenses/${id}/receipt`, {
		headers: apiHeaders(event),
		signal: event.request.signal
	}).catch(() => null);
	const scan = await withheld(upstream);
	if (scan) return scan;
	if (!upstream?.ok || !upstream.body) {
		return new Response('No receipt.', { status: upstream?.status ?? 503 });
	}
	const headers = new Headers({ 'x-content-type-options': 'nosniff' });
	for (const name of ['content-type', 'content-disposition']) {
		const value = upstream.headers.get(name);
		if (value) headers.set(name, value);
	}
	return new Response(upstream.body, { headers });
};
