import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type TypeField = components['schemas']['AssetTypeField'];

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/asset-types');
	if (response.status === 404) error(404, 'Assets are turned off.');
	if (!data) error(503, 'Could not load the asset types.');
	return { types: data };
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { errors: fieldErrors(problem), message: problemMessage(problem, fallback) });

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		let fields: TypeField[];
		try {
			fields = JSON.parse(formString(form, 'fields') || '[]') as TypeField[];
		} catch {
			return fail(400, { errors: {}, message: 'The fields could not be read.' });
		}
		const body = {
			name: formString(form, 'name'),
			fields,
			position: Number(formString(form, 'position')) || undefined,
			...(id ? { version: Number(formString(form, 'version')) } : {})
		};
		const { error: problem, response } = id
			? await api(event).PUT('/api/v1/asset-types/{id}', { params: { path: { id } }, body })
			: await api(event).POST('/api/v1/asset-types', { body });
		if (response.status !== 200 && response.status !== 201)
			return failed(response.status, problem, 'Could not save the type.');
		return { saved: true };
	},
	archive: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/asset-types/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) return failed(response.status, problem, 'Could not archive it.');
		return { saved: true };
	}
};
