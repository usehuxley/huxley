import { HUXLEY_API_URL } from '$app/env/private';
import type { RequestHandler } from '@sveltejs/kit';
import { PORTAL_COOKIE } from '#lib/session.ts';

/** Streams one of the company's invoices as a PDF, as the visitor. */
export const GET: RequestHandler = async (event) => {
	const id = encodeURIComponent(event.params.id ?? '');
	const token = event.cookies.get(PORTAL_COOKIE);
	const headers: Record<string, string> = {
		'x-requested-with': 'huxley',
		'x-forwarded-for': event.getClientAddress()
	};
	if (token) headers.cookie = `${PORTAL_COOKIE}=${token}`;
	const upstream = await fetch(`${HUXLEY_API_URL}/api/portal/v1/invoices/${id}/pdf`, {
		headers,
		signal: event.request.signal
	}).catch(() => null);
	if (!upstream?.ok || !upstream.body) {
		return new Response('Could not get the invoice.', { status: upstream?.status ?? 503 });
	}
	const out = new Headers();
	for (const name of ['content-type', 'content-disposition']) {
		const value = upstream.headers.get(name);
		if (value) out.set(name, value);
	}
	return new Response(upstream.body, { headers: out });
};
