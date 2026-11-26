import { fail, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { adoptSession } from '#lib/server/signin.ts';
import { PORTAL_COOKIE, SSO_COOKIE, safeNext } from '#lib/session.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = ({ cookies, url }) => {
	if (cookies.get(PORTAL_COOKIE)) redirect(303, safeNext(url.searchParams.get('next')));
	return { next: safeNext(url.searchParams.get('next')) };
};

/** A password as typed (not trimmed). */
const password = (form: FormData) => {
	const v = form.get('password');
	return typeof v === 'string' ? v : '';
};

const text = (form: FormData, name: string) => {
	const v = form.get(name);
	return typeof v === 'string' ? v.trim() : '';
};

export const actions: Actions = {
	link: async (event) => {
		const email = text(await event.request.formData(), 'email');
		if (!email.includes('@')) return fail(400, { message: 'Enter your work email address.' });
		// The company's own sign-in, if it has one for this address's domain.
		const sso = await api(event)
			.POST('/api/portal/v1/auth/sso/start', {
				body: { email, redirect: `${event.url.origin}/auth/sso/callback` }
			})
			.catch(() => null);
		if (sso?.data) {
			event.cookies.set(SSO_COOKIE, sso.data.state, {
				path: '/auth/sso',
				httpOnly: true,
				sameSite: 'lax',
				maxAge: 600
			});
			// The provider the API matched for this address, and only it.
			redirect(303, sso.data.url, { external: [new URL(sso.data.url).origin] });
		}
		const { response } = await api(event)
			.POST('/api/portal/v1/auth/link', { body: { email } })
			.catch(() => ({ response: { status: 503 } }));
		if (response.status === 429) {
			return fail(429, { message: 'Too many tries. Wait a minute and try again.' });
		}
		if (response.status !== 202) {
			return fail(503, { message: 'The portal can’t send links right now. Try again soon.' });
		}
		return { sent: email };
	},
	password: async (event) => {
		const form = await event.request.formData();
		const { response } = await api(event)
			.POST('/api/portal/v1/auth/password', {
				body: { email: text(form, 'email'), password: password(form) }
			})
			.catch(() => ({ response: null }));
		if (!response) return fail(503, { message: 'The portal can’t sign you in right now.' });
		if (response.status === 429) {
			return fail(429, { message: 'Too many attempts. Wait a while and try again.' });
		}
		if (
			response.status !== 200 ||
			!adoptSession(event.cookies, response.headers.get('set-cookie'))
		) {
			return fail(401, { message: 'That email and password don’t match.' });
		}
		redirect(303, safeNext(text(form, 'next')));
	}
};
