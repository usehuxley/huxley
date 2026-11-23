import { fail, redirect, type RequestEvent } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { adoptSession, clearChallenge, getChallenge, setChallenge } from '#lib/server/signin.ts';
import { SESSION_COOKIE, safeNext } from '#lib/session.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { cookies, url } = event;
	// Already signed in: skip the form. The app layout re-checks the session.
	if (cookies.get(SESSION_COOKIE)) redirect(303, safeNext(url.searchParams.get('next')));
	const client = api(event);
	const [methods, sso] = await Promise.all([
		client
			.GET('/api/v1/auth/methods', { signal: AbortSignal.timeout(5000) })
			.then((r) => r.data)
			.catch(() => undefined),
		client
			.GET('/api/v1/auth/oidc/providers', { signal: AbortSignal.timeout(5000) })
			.then((r) => r.data)
			.catch(() => undefined)
	]);
	return {
		passkeysEnabled: methods?.passkeys ?? false,
		passwordReset: methods?.passwordReset ?? false,
		notice:
			url.searchParams.get('reset') === 'done' ? 'Your password is set. Sign in with it.' : null,
		ssoProviders: sso ?? [],
		ssoError: ssoErrorMessage(url.searchParams.get('sso_error')),
		mfaMethods: getChallenge(cookies)?.methods ?? null
	};
};

/** Text for `?sso_error=` set by the single sign-on callback. */
function ssoErrorMessage(code: string | null): string | null {
	switch (code) {
		case null:
			return null;
		case 'no-account':
			return "There's no Huxley account for that identity. Ask an administrator for access.";
		case 'domain-not-allowed':
			return "That sign-in option isn't set up for your email domain.";
		case 'unverified-email':
			return "Your identity provider didn't confirm your email address.";
		case 'disabled':
			return 'Your Huxley account is disabled.';
		case 'cancelled':
			return 'Sign-in was cancelled.';
		case 'unavailable':
			return 'That sign-in option is unavailable right now.';
		default:
			return "Single sign-on couldn't be completed. Try again.";
	}
}

const UNAVAILABLE = 'Huxley is unavailable right now. Try again shortly.';
const RATE_LIMITED = 'Too many sign-in attempts. Wait a minute and try again.';

function finishSignIn(event: RequestEvent, setCookie: string | null) {
	if (!adoptSession(event.cookies, setCookie)) {
		return fail(502, { message: 'Sign-in failed unexpectedly. Try again.' });
	}
	redirect(303, safeNext(event.url.searchParams.get('next')));
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
			setChallenge(
				event.cookies,
				result.data.challenge,
				result.data.methods,
				result.data.expiresAt
			);
			return { mfa: true, methods: result.data.methods };
		}
		return finishSignIn(event, result.response.headers.get('set-cookie'));
	},

	mfa: async (event) => {
		const challenge = getChallenge(event.cookies);
		if (!challenge) {
			return fail(401, { expired: true, message: 'Sign-in timed out. Enter your password again.' });
		}
		const form = await event.request.formData();
		const code = formString(form, 'code').trim();
		const recoveryCode = formString(form, 'recoveryCode').trim();
		const methods = challenge.methods;
		if (!code && !recoveryCode) {
			return fail(400, { mfa: true, methods, message: 'Enter your code.' });
		}

		let result;
		try {
			result = await api(event).POST('/api/v1/auth/login/mfa', {
				body: recoveryCode
					? { challenge: challenge.token, recoveryCode }
					: { challenge: challenge.token, code },
				signal: AbortSignal.timeout(10000)
			});
		} catch {
			return fail(503, { mfa: true, methods, message: UNAVAILABLE });
		}

		if (result.response.status === 429) {
			return fail(429, { mfa: true, methods, message: RATE_LIMITED });
		}
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
				methods,
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
