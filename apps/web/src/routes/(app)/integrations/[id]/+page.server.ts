import { error, fail } from '@sveltejs/kit';
import { specFrom } from '#lib/integrations/spec.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const params = { path: { id: event.params.id } };
	const [integration, versions] = await Promise.all([
		client.GET('/api/v1/integrations/{id}', { params }),
		client.GET('/api/v1/integrations/{id}/versions', { params })
	]);
	if (integration.response.status === 404) error(404, 'Integration not found.');
	if (!integration.data) error(503, 'Could not load the integration.');
	return { integration: integration.data, versions: versions.data ?? [] };
};

export const actions: Actions = {
	update: async (event) => {
		const spec = await specFrom(await event.request.formData());
		if (!spec.trim()) return fail(400, { message: 'Give the new spec (paste or upload).' });
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/integrations/{id}/spec', {
			params: { path: { id: event.params.id } },
			body: { spec }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not import it.') });
		}
		return { message: `Imported as version ${data.currentVersion}.` };
	}
};
