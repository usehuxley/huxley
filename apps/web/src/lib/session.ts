/** Name of the session cookie, shared with the API. */
export const SESSION_COOKIE = 'hx_session';

/**
 * Extracts the session token and lifetime from the API's `Set-Cookie` header,
 * so the web app can set the same cookie on its own origin.
 */
export function parseSessionCookie(
	setCookie: string | null
): { token: string; maxAge: number } | null {
	if (!setCookie) return null;
	const match = setCookie.match(/(?:^|,\s*)hx_session=([^;,\s]+)/);
	if (!match) return null;
	const maxAge = Number(setCookie.match(/Max-Age=(\d+)/i)?.[1] ?? NaN);
	return { token: match[1], maxAge: Number.isFinite(maxAge) ? maxAge : 60 * 60 * 8 };
}

/**
 * Where to go after signing in. Only same-site relative paths are allowed, so
 * a crafted `?next=` link can't redirect users to another site.
 */
export function safeNext(next: string | null | undefined): string {
	if (!next || !next.startsWith('/') || next.startsWith('//') || next.startsWith('/\\')) {
		return '/';
	}
	return next;
}
