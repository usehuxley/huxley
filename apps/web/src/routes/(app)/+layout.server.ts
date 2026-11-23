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
	// Policy requires a second factor and the user has none: the API refuses
	// everything else until they add one from their profile.
	if (result.data.mfaSetupRequired && event.url.pathname !== '/profile') {
		redirect(303, '/profile');
	}
	event.depends('app:notifications');
	const unread = await api(event)
		.GET('/api/v1/notifications/unread-count')
		.then((r) => r.data?.unreadCount ?? 0)
		.catch(() => 0);
	return { user: result.data, unreadNotifications: unread };
};
