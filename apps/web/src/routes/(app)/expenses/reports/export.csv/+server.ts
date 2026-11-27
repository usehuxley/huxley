import type { RequestHandler } from '@sveltejs/kit';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';

/** Streams the approved-expenses CSV from the API. */
export const GET: RequestHandler = async (event) => {
	const status = event.url.searchParams.get('status') === 'reimbursed' ? 'reimbursed' : 'approved';
	const upstream = await fetch(`${apiUrl}/api/v1/expense-reports/export.csv?status=${status}`, {
		headers: apiHeaders(event),
		signal: event.request.signal
	}).catch(() => null);
	if (!upstream?.ok || !upstream.body) {
		return new Response('Could not export.', { status: upstream?.status ?? 503 });
	}
	const headers = new Headers();
	for (const name of ['content-type', 'content-disposition']) {
		const value = upstream.headers.get(name);
		if (value) headers.set(name, value);
	}
	return new Response(upstream.body, { headers });
};
