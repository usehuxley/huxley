import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const params = { path: { id: event.params.id } };
	const [script, versions] = await Promise.all([
		client.GET('/api/v1/scripts/{id}', { params }),
		client.GET('/api/v1/scripts/{id}/versions', { params })
	]);
	if (script.response.status === 404) error(404, 'Script not found.');
	if (!script.data) error(503, 'Could not load the script.');
	// ?version=N shows an earlier version, read-only.
	const wanted = Number(event.url.searchParams.get('version'));
	const old =
		wanted && wanted !== script.data.currentVersion
			? (
					await client.GET('/api/v1/scripts/{id}/versions/{version}', {
						params: { path: { id: event.params.id, version: wanted } }
					})
				).data
			: undefined;
	return { script: script.data, versions: versions.data ?? [], old: old ?? null };
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		let parameters: Record<string, unknown>;
		try {
			parameters = JSON.parse(formString(form, 'parameters') || '{}') as Record<string, unknown>;
		} catch {
			return fail(400, { message: 'The parameters must be valid JSON.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/scripts/{id}/versions', {
			params: { path: { id: event.params.id } },
			body: {
				source: formString(form, 'source'),
				parameters,
				notes: formString(form, 'notes') || null,
				baseVersion: Number(formString(form, 'baseVersion')) || null
			}
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not save it.') });
		}
		return { message: `Saved as version ${data.currentVersion}.` };
	},
	archive: async (event) => {
		await api(event).POST('/api/v1/scripts/{id}/archive', {
			params: { path: { id: event.params.id } }
		});
		return { message: 'Archived. Workflows can no longer use it.' };
	},
	restore: async (event) => {
		await api(event).POST('/api/v1/scripts/{id}/restore', {
			params: { path: { id: event.params.id } }
		});
		return { message: 'Restored.' };
	}
};
