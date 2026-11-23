import { json, type RequestHandler } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { api } from '#lib/server/api.ts';
import { adoptSession, clearChallenge, getChallenge } from '#lib/server/signin.ts';
import { safeNext } from '#lib/session.ts';

/** Completes the pending MFA challenge with the browser's passkey answer. */
export const POST: RequestHandler = async (event) => {
	const challenge = getChallenge(event.cookies);
	const body = (await event.request.json().catch(() => null)) as {
		ceremony?: string;
		credential?: unknown;
		next?: string;
	} | null;
	if (!challenge) {
		return json(
			{ expired: true, message: 'Sign-in timed out. Enter your password again.' },
			{ status: 401 }
		);
	}
	if (!body?.ceremony || !body.credential) {
		return json({ message: 'Invalid request.' }, { status: 400 });
	}
	const result = await api(event).POST('/api/v1/auth/login/mfa', {
		body: {
			challenge: challenge.token,
			passkey: { ceremony: body.ceremony, credential: body.credential as Record<string, never> }
		},
		signal: AbortSignal.timeout(10000)
	});
	if (!result.data) {
		if (isApiProblem(result.error) && result.error.type.endsWith('/mfa-challenge-expired')) {
			clearChallenge(event.cookies);
			return json(
				{
					expired: true,
					message: 'Sign-in timed out or had too many attempts. Enter your password again.'
				},
				{ status: 401 }
			);
		}
		return json(
			{ message: "That passkey didn't work. Try again or use another method." },
			{ status: 401 }
		);
	}
	if (!adoptSession(event.cookies, result.response.headers.get('set-cookie'))) {
		return json({ message: 'Sign-in failed unexpectedly. Try again.' }, { status: 502 });
	}
	return json({ location: safeNext(body.next) });
};
