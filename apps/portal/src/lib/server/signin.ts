import type { Cookies } from '@sveltejs/kit';
import { PORTAL_COOKIE, parsePortalCookie } from '#lib/session.ts';

/** Sets the portal session from the API's `Set-Cookie`. Whether it did. */
export function adoptSession(cookies: Cookies, setCookie: string | null): boolean {
	const session = parsePortalCookie(setCookie);
	if (!session) return false;
	cookies.set(PORTAL_COOKIE, session.token, {
		path: '/',
		httpOnly: true,
		sameSite: 'lax',
		maxAge: session.maxAge
	});
	return true;
}
