import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
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
	/** Shows what a newer spec would change; nothing is saved yet. */
	preview: async (event) => {
		const spec = await specFrom(await event.request.formData());
		if (!spec.trim()) return fail(400, { message: 'Give the new spec (paste or upload).' });
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/integrations/{id}/preview', {
			params: { path: { id: event.params.id } },
			body: { spec }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not read it.') });
		}
		return { preview: data, spec };
	},
	/** Applies a previewed spec, if the integration hasn't changed since. */
	update: async (event) => {
		const form = await event.request.formData();
		const spec = formString(form, 'spec');
		const baseVersion = Number(formString(form, 'baseVersion')) || null;
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/integrations/{id}/spec', {
			params: { path: { id: event.params.id } },
			body: { spec, baseVersion }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not apply it.') });
		}
		return { message: `Imported as version ${data.currentVersion}.` };
	},
	setCurrent: async (event) => {
		const version = Number(formString(await event.request.formData(), 'version'));
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/integrations/{id}/current', {
			params: { path: { id: event.params.id } },
			body: { version }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not switch.') });
		}
		return { message: `Version ${version} is current.` };
	}
};
