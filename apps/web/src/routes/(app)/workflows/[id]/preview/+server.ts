import { json, type RequestHandler } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';

/** Evaluates an expression against the workflow's latest run (for the editor). */
export const POST: RequestHandler = async (event) => {
	const body = (await event.request.json()) as { expression?: unknown };
	const { data, error: problem } = await api(event).POST('/api/v1/workflows/{id}/preview', {
		params: { path: { id: event.params.id ?? '' } },
		body: { expression: (body.expression ?? '') as Record<string, never> }
	});
	if (!data) return json({ error: problemMessage(problem, 'Could not preview it.') });
	return json({ value: data.value, error: data.error });
};
