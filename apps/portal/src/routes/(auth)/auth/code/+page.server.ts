import { fail, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { adoptSession } from '#lib/server/signin.ts';
import { MFA_COOKIE, safeNext } from '#lib/session.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = ({ cookies, url }) => {
	if (!cookies.get(MFA_COOKIE)) redirect(303, '/login');
	return { next: safeNext(url.searchParams.get('next')) };
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const raw = form.get('code');
		const code = typeof raw === 'string' ? raw.trim() : '';
		const challenge = event.cookies.get(MFA_COOKIE) ?? '';
		if (!code) return fail(400, { message: 'Enter the code from your app.' });
		const { response } = await api(event)
			.POST('/api/portal/v1/auth/mfa', { body: { challenge, code } })
			.catch(() => ({ response: null }));
		if (!response) return fail(503, { message: 'The portal can’t sign you in right now.' });
		if (response.status === 429) {
			return fail(429, { message: 'Too many attempts. Wait a while and sign in again.' });
		}
		if (
			response.status !== 200 ||
			!adoptSession(event.cookies, response.headers.get('set-cookie'))
		) {
			return fail(401, { message: 'That code didn’t work. Try again, or sign in again.' });
		}
		event.cookies.delete(MFA_COOKIE, { path: '/auth/code' });
		const next = form.get('next');
		redirect(303, safeNext(typeof next === 'string' ? next : null));
	}
};
