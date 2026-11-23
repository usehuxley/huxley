import type { Cookies } from '@sveltejs/kit';
import { SESSION_COOKIE, parseSessionCookie } from '#lib/session.ts';

/**
 * Holds the MFA challenge between the password and second-factor steps, with
 * the methods the user can answer it with: `<token>~<method>,<method>`.
 * HttpOnly so page scripts can't read it, and scoped to the login pages.
 */
const MFA_COOKIE = 'hx_mfa';
const MFA_COOKIE_OPTIONS = { path: '/login', httpOnly: true, sameSite: 'strict' } as const;

/**
 * Binds a single sign-on attempt to the browser that started it. `Lax`, not
 * `Strict`: the provider's redirect back is a cross-site navigation, which
 * `Strict` cookies don't accompany.
 */
export const OIDC_COOKIE = 'hx_oidc';
export const OIDC_COOKIE_OPTIONS = {
	path: '/login/oidc',
	httpOnly: true,
	sameSite: 'lax'
} as const;

export type MfaMethod = 'totp' | 'passkey' | 'recoveryCode';

export function setChallenge(
	cookies: Cookies,
	challenge: string,
	methods: MfaMethod[],
	expiresAt: string
) {
	const seconds = Math.floor((Date.parse(expiresAt) - Date.now()) / 1000);
	cookies.set(MFA_COOKIE, `${challenge}~${methods.join(',')}`, {
		...MFA_COOKIE_OPTIONS,
		maxAge: Math.max(seconds, 1)
	});
}

export function getChallenge(cookies: Cookies): { token: string; methods: MfaMethod[] } | null {
	const value = cookies.get(MFA_COOKIE);
	if (!value) return null;
	const [token, methods = ''] = value.split('~');
	return { token, methods: methods.split(',').filter(Boolean) as MfaMethod[] };
}

export function clearChallenge(cookies: Cookies) {
	cookies.delete(MFA_COOKIE, MFA_COOKIE_OPTIONS);
}

/**
 * Copies the API's session cookie onto this origin and ends any MFA step.
 * Returns false if the API response carried no session.
 */
export function adoptSession(cookies: Cookies, setCookie: string | null): boolean {
	const session = parseSessionCookie(setCookie);
	if (!session) return false;
	clearChallenge(cookies);
	cookies.set(SESSION_COOKIE, session.token, {
		path: '/',
		httpOnly: true,
		sameSite: 'lax',
		maxAge: session.maxAge
	});
	return true;
}
