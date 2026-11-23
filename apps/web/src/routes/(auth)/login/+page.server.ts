import { fail, redirect, type Cookies, type RequestEvent } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { SESSION_COOKIE, parseSessionCookie, safeNext } from '#lib/session.ts';
import type { Actions, PageServerLoad } from './$types';

/**
 * Holds the MFA challenge between the password and code steps. HttpOnly so
 * page scripts can't read it, and scoped to the login page.
 */
const MFA_COOKIE = 'hx_mfa';
const MFA_COOKIE_OPTIONS = { path: '/login', httpOnly: true, sameSite: 'strict' } as const;

export const load: PageServerLoad = ({ cookies, url }) => {
	// Already signed in: skip the form. The app layout re-checks the session.
	if (cookies.get(SESSION_COOKIE)) redirect(303, safeNext(url.searchParams.get('next')));
	return { mfaPending: Boolean(cookies.get(MFA_COOKIE)) };
};

const UNAVAILABLE = 'Huxley is unavailable right now. Try again shortly.';
const RATE_LIMITED = 'Too many sign-in attempts. Wait a minute and try again.';

/** Copies the API's session cookie onto this origin and leaves the login page. */
function finishSignIn(event: RequestEvent, setCookie: string | null) {
	const session = parseSessionCookie(setCookie);
	if (!session) return fail(502, { message: 'Sign-in failed unexpectedly. Try again.' });
	event.cookies.delete(MFA_COOKIE, MFA_COOKIE_OPTIONS);
	event.cookies.set(SESSION_COOKIE, session.token, {
		path: '/',
		httpOnly: true,
		sameSite: 'lax',
		maxAge: session.maxAge
	});
	redirect(303, safeNext(event.url.searchParams.get('next')));
}

function clearChallenge(cookies: Cookies) {
	cookies.delete(MFA_COOKIE, MFA_COOKIE_OPTIONS);
}

export const actions: Actions = {
	password: async (event) => {
		const form = await event.request.formData();
		const email = formString(form, 'email').trim();
		const password = formString(form, 'password');
		if (!email || !password) {
			return fail(400, { email, message: 'Enter your email and password.' });
		}

		let result;
		try {
			result = await api(event).POST('/api/v1/auth/login', {
				body: { email, password },
				signal: AbortSignal.timeout(10000)
			});
		} catch {
			return fail(503, { email, message: UNAVAILABLE });
		}

		if (result.response.status === 429) return fail(429, { email, message: RATE_LIMITED });
		if (!result.data) return fail(401, { email, message: 'Invalid email or password.' });

		if (result.data.status === 'mfaRequired') {
			const seconds = Math.floor((Date.parse(result.data.expiresAt) - Date.now()) / 1000);
			event.cookies.set(MFA_COOKIE, result.data.challenge, {
				...MFA_COOKIE_OPTIONS,
				maxAge: Math.max(seconds, 1)
			});
			return { mfa: true };
		}
		return finishSignIn(event, result.response.headers.get('set-cookie'));
	},

	mfa: async (event) => {
		const challenge = event.cookies.get(MFA_COOKIE);
		if (!challenge) {
			return fail(401, { expired: true, message: 'Sign-in timed out. Enter your password again.' });
		}
		const form = await event.request.formData();
		const code = formString(form, 'code').trim();
		const recoveryCode = formString(form, 'recoveryCode').trim();
		if (!code && !recoveryCode) {
			return fail(400, { mfa: true, message: 'Enter the code from your authenticator app.' });
		}

		let result;
		try {
			result = await api(event).POST('/api/v1/auth/login/mfa', {
				body: recoveryCode ? { challenge, recoveryCode } : { challenge, code },
				signal: AbortSignal.timeout(10000)
			});
		} catch {
			return fail(503, { mfa: true, message: UNAVAILABLE });
		}

		if (result.response.status === 429) return fail(429, { mfa: true, message: RATE_LIMITED });
		if (!result.data) {
			const expired =
				isApiProblem(result.error) && result.error.type.endsWith('/mfa-challenge-expired');
			if (expired) {
				clearChallenge(event.cookies);
				return fail(401, {
					expired: true,
					message: 'Sign-in timed out or had too many wrong codes. Enter your password again.'
				});
			}
			return fail(401, {
				mfa: true,
				message: recoveryCode ? 'That recovery code is not valid.' : 'That code is not valid.'
			});
		}
		return finishSignIn(event, result.response.headers.get('set-cookie'));
	},

	restart: ({ cookies }) => {
		clearChallenge(cookies);
		return { restarted: true };
	}
};
