import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';

/** Starts adding a passkey: re-checks the password, returns WebAuthn options. */
export const POST: RequestHandler = async (event) => {
	const body = (await event.request.json().catch(() => null)) as { password?: string } | null;
	const { data, error, response } = await api(event).POST('/api/v1/auth/passkeys/options', {
		body: { password: body?.password ?? '' }
	});
	if (!data) {
		return json(
			{ message: problemMessage(error, 'Could not start adding a passkey.') },
			{ status: response.status }
		);
	}
	return json(data, { headers: { 'cache-control': 'no-store' } });
};
