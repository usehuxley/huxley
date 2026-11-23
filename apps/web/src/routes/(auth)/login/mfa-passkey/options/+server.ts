import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { getChallenge } from '#lib/server/signin.ts';

/** Options for answering the pending MFA challenge with a passkey. */
export const POST: RequestHandler = async (event) => {
	const challenge = getChallenge(event.cookies);
	if (!challenge) {
		return json({ message: 'Sign-in timed out. Enter your password again.' }, { status: 401 });
	}
	const { data, response } = await api(event).POST('/api/v1/auth/login/mfa/passkey', {
		body: { challenge: challenge.token },
		signal: AbortSignal.timeout(10000)
	});
	if (!data) {
		return json(
			{ message: 'Sign-in timed out. Enter your password again.' },
			{ status: response.status }
		);
	}
	return json(data, { headers: { 'cache-control': 'no-store' } });
};
