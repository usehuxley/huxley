import type { RequestHandler } from '@sveltejs/kit';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';

/** Streams an audit export from the API (format, action, from, to pass through). */
export const GET: RequestHandler = async (event) => {
	const query = new URLSearchParams();
	for (const name of ['format', 'action', 'from', 'to']) {
		const value = event.url.searchParams.get(name)?.trim();
		if (value) query.set(name, value);
	}
	const upstream = await fetch(`${apiUrl}/api/v1/audit/export?${query.toString()}`, {
		headers: apiHeaders(event),
		signal: event.request.signal
	}).catch(() => null);
	if (!upstream?.ok || !upstream.body) {
		return new Response('Could not export the audit log.', { status: upstream?.status ?? 503 });
	}
	const headers = new Headers();
	for (const name of ['content-type', 'content-disposition', 'cache-control']) {
		const value = upstream.headers.get(name);
		if (value) headers.set(name, value);
	}
	return new Response(upstream.body, { headers });
};
