import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** The server's problems with a draft document, not saved (for the editor's
 * inline checks). */
export const POST: RequestHandler = async (event) => {
	const body = (await event.request.json()) as { document?: unknown };
	const { data } = await api(event).POST('/api/v1/workflows/validate', {
		body: { document: (body.document ?? {}) as Record<string, never> }
	});
	return json({ problems: data?.diagnostics ?? [] });
};
