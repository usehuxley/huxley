import { fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { SESSION_COOKIE, parseSessionCookie, safeNext } from '#lib/session.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = ({ cookies, url }) => {
	// Already signed in: skip the form. The app layout re-checks the session.
	if (cookies.get(SESSION_COOKIE)) redirect(303, safeNext(url.searchParams.get('next')));
};

export const actions: Actions = {
	default: async (event) => {
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
			return fail(503, { email, message: 'Huxley is unavailable right now. Try again shortly.' });
		}

		if (result.response.status === 429) {
			return fail(429, {
				email,
				message: 'Too many sign-in attempts. Wait a minute and try again.'
			});
		}
		if (!result.data) {
			return fail(401, { email, message: 'Invalid email or password.' });
		}

		const session = parseSessionCookie(result.response.headers.get('set-cookie'));
		if (!session) {
			return fail(502, { email, message: 'Sign-in failed unexpectedly. Try again.' });
		}
		event.cookies.set(SESSION_COOKIE, session.token, {
			path: '/',
			httpOnly: true,
			sameSite: 'lax',
			maxAge: session.maxAge
		});
		redirect(303, safeNext(event.url.searchParams.get('next')));
	}
};
