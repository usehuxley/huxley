import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { parent } = event;
	const { user } = await parent();
	const client = api(event);
	// Your organization's templates: your first organization's.
	const orgId = (await client.GET('/api/v1/orgs')).data?.[0]?.id;
	const { data } = await client.GET('/api/v1/templates', {
		params: { query: orgId ? { orgId } : {} }
	});
	if (!data) error(503, 'Could not load templates.');
	return { templates: data, canPublish: user.grantedPermissions.includes('workflow:publish') };
};

export const actions: Actions = {
	delete: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { response, error: problem } = await api(event).DELETE('/api/v1/templates/{id}', {
			params: { path: { id } }
		});
		if (!response.ok) {
			return fail(response.status, { message: problemMessage(problem, 'Could not delete it.') });
		}
		return { message: 'Template deleted.' };
	}
};
