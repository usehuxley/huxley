import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';

/** Finishes adding a passkey with the browser's answer. */
export const POST: RequestHandler = async (event) => {
	const body = (await event.request.json().catch(() => null)) as {
		ceremony?: string;
		name?: string;
		credential?: unknown;
	} | null;
	if (!body?.ceremony || !body.credential) {
		return json({ message: 'Invalid request.' }, { status: 400 });
	}
	const { data, error, response } = await api(event).POST('/api/v1/auth/passkeys', {
		body: {
			ceremony: body.ceremony,
			name: body.name ?? '',
			// Passed through untouched; the API validates it.
			credential: body.credential as Record<string, never>
		}
	});
	if (!data) {
		return json(
			{ message: problemMessage(error, 'Could not add the passkey.') },
			{ status: response.status }
		);
	}
	return json(data, { status: 201, headers: { 'cache-control': 'no-store' } });
};
