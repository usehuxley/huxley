import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type MappingInput = components['schemas']['AssetMappingInput'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [mappings, datasets, types] = await Promise.all([
		client.GET('/api/v1/asset-mappings'),
		client.GET('/api/v1/datasets'),
		client.GET('/api/v1/asset-types')
	]);
	if (mappings.response.status === 404) error(404, 'Assets are turned off.');
	if (!mappings.data)
		error(mappings.response.status === 403 ? 403 : 503, 'Could not load the mappings.');
	return {
		mappings: mappings.data,
		datasets: (datasets.data ?? []).map((d) => ({
			id: d.id,
			name: d.name,
			columns: d.columns.map((c) => c.name)
		})),
		types: (types.data ?? []).filter((t) => !t.archived)
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { errors: fieldErrors(problem), message: problemMessage(problem, fallback) });

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		let body: MappingInput;
		try {
			body = JSON.parse(formString(form, 'mapping')) as MappingInput;
		} catch {
			return fail(400, { errors: {}, message: 'The mapping could not be read.' });
		}
		const { error: problem, response } = id
			? await api(event).PUT('/api/v1/asset-mappings/{id}', { params: { path: { id } }, body })
			: await api(event).POST('/api/v1/asset-mappings', { body });
		if (response.status !== 200 && response.status !== 201)
			return failed(response.status, problem, 'Could not save the mapping.');
		return { saved: true };
	},
	run: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const preview = form.get('preview') === 'true';
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/asset-mappings/{id}/run', {
			params: { path: { id }, query: { preview } }
		});
		if (!data) return failed(response.status, problem, 'The sync failed.');
		return { result: data, ranId: id };
	},
	delete: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/asset-mappings/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) return failed(response.status, problem, 'Could not delete it.');
		return { saved: true };
	}
};
