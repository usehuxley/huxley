import { redirect, type Cookies } from '@sveltejs/kit';
import { MFA_COOKIE, PORTAL_COOKIE, parsePortalCookie } from '#lib/session.ts';

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

/**
 * A sign-in answered 202: a two-factor code is needed. Keeps the challenge
 * in a cookie and goes to the code page.
 */
export function toCodeStep(cookies: Cookies, body: unknown, next: string) {
	const challenge = (body as { challenge?: unknown } | undefined)?.challenge;
	if (typeof challenge !== 'string') return;
	cookies.set(MFA_COOKIE, challenge, {
		path: '/auth/code',
		httpOnly: true,
		sameSite: 'strict',
		maxAge: 300
	});
	redirect(303, `/auth/code?next=${encodeURIComponent(next)}`);
}
