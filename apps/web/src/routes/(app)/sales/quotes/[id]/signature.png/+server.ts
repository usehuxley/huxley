import type { RequestHandler } from '@sveltejs/kit';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';

/** The signature drawn when the quote was accepted in the portal. */
export const GET: RequestHandler = async (event) => {
	const id = encodeURIComponent(event.params.id ?? '');
	const upstream = await fetch(`${apiUrl}/api/v1/quotes/${id}/signature.png`, {
		headers: apiHeaders(event),
		signal: event.request.signal
	}).catch(() => null);
	if (!upstream?.ok || !upstream.body)
		return new Response(null, { status: upstream?.status ?? 503 });
	return new Response(upstream.body, {
		headers: { 'content-type': 'image/png', 'cache-control': 'private, max-age=3600' }
	});
};
