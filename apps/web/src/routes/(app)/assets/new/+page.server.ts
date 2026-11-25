import { error, fail, redirect } from '@sveltejs/kit';
import { assetBody } from '#lib/assets/form.ts';
import { orgOptions } from '#lib/assets/server.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { user } = await event.parent();
	if (!user.grantedPermissions.includes('asset:write')) error(403, 'You may not add assets.');
	const client = api(event);
	const [types, orgs] = await Promise.all([
		client.GET('/api/v1/asset-types'),
		client.GET('/api/v1/orgs')
	]);
	if (types.response.status === 404) error(404, 'Assets are turned off.');
	const orgId = event.url.searchParams.get('org') ?? '';
	const org = (orgs.data ?? []).find((o) => o.id === orgId);
	return {
		orgId: org ? orgId : '',
		orgName: org?.name ?? '',
		orgs: (orgs.data ?? []).map((o) => ({ value: o.id, label: o.name })),
		types: types.data ?? [],
		...(org ? await orgOptions(event, orgId) : { sites: [], contacts: [] })
	};
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const orgId = event.url.searchParams.get('org') ?? '';
		const types = (await api(event).GET('/api/v1/asset-types')).data ?? [];
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/assets', {
			body: { orgId, ...assetBody(form, types) }
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
