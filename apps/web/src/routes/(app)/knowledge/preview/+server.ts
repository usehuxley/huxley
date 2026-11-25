import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** The editor's preview: markdown rendered as the article will show it. */
export const POST: RequestHandler = async (event) => {
	const { body } = (await event.request.json()) as { body?: unknown };
	const { data, response } = await api(event).POST('/api/v1/kb/preview', {
		body: { body: typeof body === 'string' ? body : '' }
	});
	return json(data ?? { html: '' }, { status: data ? 200 : response.status });
};
