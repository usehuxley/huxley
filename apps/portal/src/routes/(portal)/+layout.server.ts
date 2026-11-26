import { redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { PORTAL_COOKIE } from '#lib/session.ts';
import type { LayoutServerLoad } from './$types';

/** Who's signed in; everyone else goes to sign in. */
export const load: LayoutServerLoad = async (event) => {
	const toLogin = () => {
		const next = event.url.pathname + event.url.search;
		redirect(303, next === '/' ? '/login' : `/login?next=${encodeURIComponent(next)}`);
	};
	if (!event.cookies.get(PORTAL_COOKIE)) toLogin();
	const me = await api(event)
		.GET('/api/portal/v1/me')
		.catch(() => null);
	if (!me?.data) {
		if (me?.response.status === 401) event.cookies.delete(PORTAL_COOKIE, { path: '/' });
		toLogin();
	}
	// Their company requires two-factor sign-in: set it up first.
	if (me!.data!.mfaSetupRequired && event.url.pathname !== '/profile/security') {
		redirect(303, '/profile/security');
	}
	return { me: me!.data! };
};
