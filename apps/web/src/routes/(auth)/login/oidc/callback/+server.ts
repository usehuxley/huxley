import { redirect, type RequestHandler } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { api } from '#lib/server/api.ts';
import { OIDC_COOKIE, OIDC_COOKIE_OPTIONS, adoptSession } from '#lib/server/signin.ts';
import { safeNext } from '#lib/session.ts';

/** Where the identity provider sends the browser back. */
export const GET: RequestHandler = async (event) => {
	const { url, cookies } = event;
	const expected = cookies.get(OIDC_COOKIE);
	cookies.delete(OIDC_COOKIE, OIDC_COOKIE_OPTIONS);
	const state = url.searchParams.get('state');
	const code = url.searchParams.get('code');

	// The provider reported an error (e.g. the user cancelled).
	if (url.searchParams.get('error')) redirect(303, '/login?sso_error=cancelled');
	// The sign-in must have started in this browser: otherwise someone could
	// sign the victim in to the attacker's account.
	if (!state || !code || !expected || state !== expected) {
		redirect(303, '/login?sso_error=failed');
	}

	const result = await api(event)
		.POST('/api/v1/auth/oidc/callback', {
			body: { state, code },
			signal: AbortSignal.timeout(15000)
		})
		.catch(() => undefined);
	if (!result?.data || !adoptSession(cookies, result.response.headers.get('set-cookie'))) {
		const type = result && isApiProblem(result.error) ? result.error.type : '';
		const reason = type.split('/problems/sso-')[1] ?? 'failed';
		redirect(303, `/login?sso_error=${encodeURIComponent(reason)}`);
	}
	const next = result.data.status === 'signedIn' ? result.data.next : undefined;
	redirect(303, safeNext(next));
};
