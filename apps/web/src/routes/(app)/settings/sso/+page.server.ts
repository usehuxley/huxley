import { error, fail, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors, parseProviderForm } from '#lib/server/sso.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/security/sso/providers');
	if (response.status === 403) error(403, 'You cannot manage single sign-on.');
	if (!data) error(503, 'Could not load identity providers.');
	return { providers: data, redirectUri: `${event.url.origin}/login/oidc/callback` };
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/security/sso/providers', { body: parseProviderForm(form) });
		if (!data) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not add the provider.')
			});
		}
		redirect(303, `/settings/sso/${data.id}`);
	}
};
