import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { assetBody } from '#lib/assets/form.ts';
import { orgOptions } from '#lib/assets/server.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [asset, types] = await Promise.all([
		client.GET('/api/v1/assets/{id}', { params: { path: { id: event.params.id } } }),
		client.GET('/api/v1/asset-types')
	]);
	if (!asset.data) error(asset.response.status === 404 ? 404 : 503, 'Could not load the asset.');
	if (!asset.data.canWrite) error(403, 'You may not change this asset.');
	return {
		asset: asset.data,
		types: types.data ?? [],
		...(await orgOptions(event, asset.data.orgId))
	};
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const types = (await api(event).GET('/api/v1/asset-types')).data ?? [];
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/assets/{id}', {
			params: { path: { id: event.params.id } },
			body: { version: Number(formString(form, 'version')), ...assetBody(form, types) }
		});
		if (!data) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not save the asset.')
			});
		}
		redirect(303, `/assets/${data.id}`);
	}
};
