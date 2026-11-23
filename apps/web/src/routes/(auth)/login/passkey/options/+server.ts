import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** Starts passwordless sign-in: returns WebAuthn options for the browser. */
export const POST: RequestHandler = async (event) => {
	const { data, response } = await api(event).POST('/api/v1/auth/login/passkey/options', {
		signal: AbortSignal.timeout(10000)
	});
	if (!data) {
		const message =
			response.status === 429
				? 'Too many sign-in attempts. Wait a minute and try again.'
				: 'Passkey sign-in is unavailable right now.';
		return json({ message }, { status: response.status });
	}
	return json(data, { headers: { 'cache-control': 'no-store' } });
};
