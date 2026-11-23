import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { adoptSession } from '#lib/server/signin.ts';
import { safeNext } from '#lib/session.ts';

/** Finishes passwordless sign-in with the browser's passkey answer. */
export const POST: RequestHandler = async (event) => {
	const body = (await event.request.json().catch(() => null)) as {
		ceremony?: string;
		credential?: unknown;
		next?: string;
	} | null;
	if (!body?.ceremony || !body.credential) {
		return json({ message: 'Invalid request.' }, { status: 400 });
	}
	const result = await api(event).POST('/api/v1/auth/login/passkey', {
		// The credential is passed through untouched; the API validates it.
		body: { ceremony: body.ceremony, credential: body.credential as Record<string, never> },
		signal: AbortSignal.timeout(10000)
	});
	if (!result.data || !adoptSession(event.cookies, result.response.headers.get('set-cookie'))) {
		return json(
			{ message: "That passkey didn't work. Try another, or sign in with your password." },
			{ status: 401 }
		);
	}
	return json({ location: safeNext(body.next) });
};
