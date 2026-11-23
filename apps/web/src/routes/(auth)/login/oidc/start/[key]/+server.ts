import { redirect, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { safeNext } from '#lib/session.ts';
import { OIDC_COOKIE, OIDC_COOKIE_OPTIONS } from '#lib/server/signin.ts';

/**
 * Starts single sign-on: asks the API for the provider's authorization URL,
 * binds the sign-in to this browser with a cookie, and redirects there.
 *
 * A GET link rather than a form: the CSP's `form-action 'self'` would block a
 * form submission that redirects to the provider.
 */
export const GET: RequestHandler = async (event) => {
	const next = safeNext(event.url.searchParams.get('next'));
	const { data } = await api(event)
		.POST('/api/v1/auth/oidc/start', {
			body: { provider: event.params.key ?? '', next },
			signal: AbortSignal.timeout(10000)
		})
		.catch(() => ({ data: undefined }));
	if (!data) redirect(303, '/login?sso_error=unavailable');
	event.cookies.set(OIDC_COOKIE, data.state, { ...OIDC_COOKIE_OPTIONS, maxAge: 600 });
	// SvelteKit's redirect() refuses other origins on purpose; sending the
	// browser to the identity provider is the one place we mean to.
	return new Response(null, {
		status: 303,
		headers: { location: data.authorizationUrl, 'cache-control': 'no-store' }
	});
};
