import { HUXLEY_API_URL } from '$app/env/private';
import type { RequestEvent } from '@sveltejs/kit';
import { createHuxleyClient } from '@usehuxley/api-client';
import { SESSION_COOKIE } from '#lib/session.ts';

type ApiEvent = Pick<RequestEvent, 'fetch' | 'cookies' | 'getClientAddress' | 'request'>;

/**
 * Server-side API client acting for the current browser request.
 *
 * Forwards the session cookie, the client's address (the API trusts it only
 * when `HUXLEY_HTTP__TRUST_FORWARDED_FOR` is set), the user agent for audit
 * records, and the anti-forgery header the API requires on state changes.
 */
export function api(event: ApiEvent) {
	const headers: Record<string, string> = {
		'x-requested-with': 'huxley',
		'x-forwarded-for': event.getClientAddress()
	};
	const token = event.cookies.get(SESSION_COOKIE);
	if (token) headers.cookie = `${SESSION_COOKIE}=${token}`;
	const userAgent = event.request.headers.get('user-agent');
	if (userAgent) headers['user-agent'] = userAgent;

	return createHuxleyClient(HUXLEY_API_URL, { fetch: event.fetch, headers });
}
