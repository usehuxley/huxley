import { error, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { SESSION_COOKIE } from '#lib/session.ts';
import type { LayoutServerLoad } from './$types';

/** Every page in the app requires a signed-in user. */
export const load: LayoutServerLoad = async (event) => {
	let result;
	try {
		result = await api(event).GET('/api/v1/auth/me', { signal: AbortSignal.timeout(5000) });
	} catch {
		error(503, 'The Huxley API is unavailable. Try again shortly.');
	}

	if (result.response.status === 401) {
		event.cookies.delete(SESSION_COOKIE, { path: '/' });
		const next = event.url.pathname + event.url.search;
		redirect(303, next === '/' ? '/login' : `/login?next=${encodeURIComponent(next)}`);
	}
	if (!result.data) {
		error(503, 'The Huxley API is unavailable. Try again shortly.');
	}
	return { user: result.data };
};
