import type { RequestHandler } from '@sveltejs/kit';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';

/** Streams a dashboard export from the API (format, range and org pass through). */
export const GET: RequestHandler = async (event) => {
	const query = new URLSearchParams();
	for (const [name, as] of [
		['format', 'format'],
		['range', 'range'],
		['org', 'orgId']
	] as const) {
		const value = event.url.searchParams.get(name)?.trim();
		if (value) query.set(as, value);
	}
	const id = encodeURIComponent(event.params.id ?? '');
	const upstream = await fetch(`${apiUrl}/api/v1/dashboards/${id}/export?${query.toString()}`, {
		headers: apiHeaders(event),
		signal: event.request.signal
	}).catch(() => null);
	if (!upstream?.ok || !upstream.body) {
		return new Response('Could not export the dashboard.', { status: upstream?.status ?? 503 });
	}
	const headers = new Headers();
	for (const name of ['content-type', 'content-disposition']) {
		const value = upstream.headers.get(name);
		if (value) headers.set(name, value);
	}
	return new Response(upstream.body, { headers });
};
