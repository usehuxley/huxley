import { redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { SESSION_COOKIE } from '#lib/session.ts';
import type { RequestHandler } from './$types';

/** Signs out: revokes the session in the API, then clears the cookie. */
export const POST: RequestHandler = async (event) => {
	try {
		await api(event).POST('/api/v1/auth/logout', { signal: AbortSignal.timeout(5000) });
	} catch {
		// Still clear the local cookie; the session expires on its own.
	}
	event.cookies.delete(SESSION_COOKIE, { path: '/' });
	redirect(303, '/login');
};
