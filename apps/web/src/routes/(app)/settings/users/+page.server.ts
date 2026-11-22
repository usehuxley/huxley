import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const cursor = event.url.searchParams.get('cursor') ?? undefined;
	const { data, response } = await api(event).GET('/api/v1/users', {
		params: { query: { cursor, limit: 50 } }
	});
	if (response.status === 403) error(403, 'You do not have access to users.');
	if (!data) error(503, 'Could not load users.');
	return { page: data, cursor };
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const email = formString(form, 'email');
		const displayName = formString(form, 'displayName');
		const password = formString(form, 'password');
		const { data, error: problem } = await api(event).POST('/api/v1/users', {
			body: { email, displayName, password: password || null }
		});
		if (!data) {
			return fail(400, {
				email,
				displayName,
				message: problemMessage(problem, 'Could not create the user.')
			});
		}
		return {
			created: { id: data.user.id, email: data.user.email },
			generatedPassword: data.generatedPassword ?? null
		};
	}
};
