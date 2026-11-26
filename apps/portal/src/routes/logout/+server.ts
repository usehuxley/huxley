import { redirect, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { PORTAL_COOKIE } from '#lib/session.ts';

/** Signs out: ends the session at the API and clears the cookie. */
export const POST: RequestHandler = async (event) => {
	await api(event)
		.POST('/api/portal/v1/auth/logout')
		.catch(() => null);
	event.cookies.delete(PORTAL_COOKIE, { path: '/' });
	redirect(303, '/login');
};
