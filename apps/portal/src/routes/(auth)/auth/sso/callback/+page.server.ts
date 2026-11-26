import { redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { adoptSession } from '#lib/server/signin.ts';
import { SSO_COOKIE } from '#lib/session.ts';
import type { PageServerLoad } from './$types';

/**
 * The company's identity provider sends people back here. The sign-in
 * finishes only for the browser that started it (the state cookie).
 */
export const load: PageServerLoad = async (event) => {
	const { url, cookies } = event;
	const state = url.searchParams.get('state') ?? '';
	const code = url.searchParams.get('code') ?? '';
	const expected = cookies.get(SSO_COOKIE);
	cookies.delete(SSO_COOKIE, { path: '/auth/sso' });
	if (url.searchParams.get('error')) {
		return { message: 'Your organization didn’t sign you in. Try again, or use a sign-in link.' };
	}
	if (!state || !code || !expected || state !== expected) {
		return { message: 'This sign-in didn’t start here, or took too long. Try again.' };
	}
	const { response, error } = await api(event)
		.POST('/api/portal/v1/auth/sso/callback', { body: { state, code } })
		.catch(() => ({ response: null, error: undefined }));
	if (!response || response.status !== 200) {
		const detail = (error as { detail?: string } | undefined)?.detail;
		return { message: detail ?? 'Signing in with your organization didn’t work. Try again.' };
	}
	if (!adoptSession(cookies, response.headers.get('set-cookie'))) {
		return { message: 'Signing in with your organization didn’t work. Try again.' };
	}
	redirect(303, '/');
};
