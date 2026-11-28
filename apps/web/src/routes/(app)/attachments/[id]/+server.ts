import type { RequestHandler } from '@sveltejs/kit';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';
import { withheld } from '#lib/server/withheld.ts';

/** Downloads an attachment through the app, keeping the API's safe headers. */
export const GET: RequestHandler = async (event) => {
	const upstream = await fetch(
		`${apiUrl}/api/v1/attachments/${encodeURIComponent(event.params.id ?? '')}/content`,
		{ headers: apiHeaders(event), signal: event.request.signal }
	).catch(() => null);
	const scan = await withheld(upstream);
	if (scan) return scan;
	if (!upstream?.ok || !upstream.body) {
		return new Response('Not found', { status: upstream?.status === 403 ? 403 : 404 });
	}
	const headers = new Headers();
	for (const name of [
		'content-type',
		'content-disposition',
		'content-length',
		'x-content-type-options',
		'content-security-policy',
		'cache-control'
	]) {
		const value = upstream.headers.get(name);
		if (value) headers.set(name, value);
	}
	return new Response(upstream.body, { headers });
};
