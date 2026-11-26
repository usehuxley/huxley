/** The portal's session cookie, shared with the API (ADR-040). */
export const PORTAL_COOKIE = 'hx_portal_session';

/**
 * Binds a sign-in with the company's identity provider to this browser.
 * `Lax`: the provider's redirect back is a cross-site navigation.
 */
export const SSO_COOKIE = 'hx_portal_sso';

/** Holds the two-factor challenge between the first factor and the code. */
export const MFA_COOKIE = 'hx_portal_mfa';

/**
 * The session token and lifetime from the API's `Set-Cookie` header, so the
 * portal can set the same cookie on its own origin.
 */
export function parsePortalCookie(
	setCookie: string | null
): { token: string; maxAge: number } | null {
	if (!setCookie) return null;
	const match = setCookie.match(/(?:^|,\s*)hx_portal_session=([^;,\s]+)/);
	if (!match) return null;
	const maxAge = Number(setCookie.match(/Max-Age=(\d+)/i)?.[1] ?? NaN);
	return { token: match[1], maxAge: Number.isFinite(maxAge) ? maxAge : 60 * 60 * 24 };
}

/**
 * Where to go after signing in. Only same-site relative paths, so a crafted
 * `?next=` link can't send people to another site.
 */
export function safeNext(next: string | null | undefined): string {
	if (!next || !next.startsWith('/') || next.startsWith('//') || next.startsWith('/\\')) {
		return '/';
	}
	return next;
}
