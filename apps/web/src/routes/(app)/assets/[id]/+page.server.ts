import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const path = { params: { path: { id: event.params.id } } };
	const [asset, impact, types] = await Promise.all([
		client.GET('/api/v1/assets/{id}', path),
		client.GET('/api/v1/assets/{id}/impact', path),
		client.GET('/api/v1/asset-types')
	]);
	if (!asset.data) error(asset.response.status === 404 ? 404 : 503, 'Could not load the asset.');
	// Assets it can relate to: those of its organization (search narrows).
	const q = event.url.searchParams.get('relate')?.trim() ?? '';
	const candidates = asset.data.canWrite
		? (
				(
					await client.GET('/api/v1/assets', {
						params: {
							query: { orgId: asset.data.orgId, q: q || undefined, inUse: true, limit: 50 }
						}
					})
				).data?.items ?? []
			)
				.filter((a) => a.id !== asset.data.id)
				.map((a) => ({ value: a.id, label: `${a.name} (${a.typeName})` }))
		: [];
	return {
		asset: asset.data,
		impact: impact.data ?? [],
		type: (types.data ?? []).find((t) => t.id === asset.data.typeId) ?? null,
		candidates
	};
};

export const actions: Actions = {
	relate: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST(
			'/api/v1/assets/{id}/relationships',
			{
				params: { path: { id: event.params.id } },
				body: { kind: formString(form, 'kind'), toId: formString(form, 'toId') }
			}
		);
		if (response.status !== 204)
			return fail(response.status, { message: problemMessage(problem, 'Could not relate them.') });
		return { message: 'Related.' };
	},
	unrelate: async (event) => {
		const id = formString(await event.request.formData(), 'relationshipId');
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/asset-relationships/{id}',
			{
				params: { path: { id } }
			}
		);
		if (response.status !== 204)
			return fail(response.status, { message: problemMessage(problem, 'Could not remove it.') });
		return { message: 'Removed.' };
	},
	delete: async (event) => {
		const { error: problem, response } = await api(event).DELETE('/api/v1/assets/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (response.status !== 204)
			return fail(response.status, { message: problemMessage(problem, 'Could not delete it.') });
		redirect(303, '/assets');
	}
};
