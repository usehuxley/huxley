import { HUXLEY_API_URL } from '$app/env/private';
import type { RequestEvent } from '@sveltejs/kit';
import { createHuxleyClient } from '@usehuxley/api-client';
import { PORTAL_COOKIE } from '#lib/session.ts';

type ApiEvent = Pick<RequestEvent, 'fetch' | 'cookies' | 'getClientAddress' | 'request'>;

/**
 * Server-side API client acting for the current portal visitor. Forwards
 * only the portal session cookie (never a staff one), the client's address,
 * the user agent, and the anti-forgery header.
 */
export function api(event: ApiEvent) {
	const headers: Record<string, string> = {
		'x-requested-with': 'huxley',
		'x-forwarded-for': event.getClientAddress(),
		// The same, for deployments where the API reads it from a header a
		// router sets (HUXLEY_HTTP__CLIENT_IP_HEADER; the Azure template).
		'x-huxley-client-ip': event.getClientAddress()
	};
	const token = event.cookies.get(PORTAL_COOKIE);
	if (token) headers.cookie = `${PORTAL_COOKIE}=${token}`;
	const userAgent = event.request.headers.get('user-agent');
	if (userAgent) headers['user-agent'] = userAgent;
	return createHuxleyClient(HUXLEY_API_URL, { fetch: event.fetch, headers });
}
