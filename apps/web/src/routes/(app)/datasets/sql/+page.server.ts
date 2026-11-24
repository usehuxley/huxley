import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { user } = await event.parent();
	if (!user.grantedPermissions.includes('platform:datasets:sql')) {
		error(403, 'The SQL console is for instance administrators.');
	}
	const { data } = await api(event).GET('/api/v1/datasets');
	return { datasets: (data ?? []).map((d) => ({ id: d.id, key: d.key, name: d.name })) };
};

export const actions: Actions = {
	run: async (event) => {
		const query = formString(await event.request.formData(), 'query');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/datasets/sql', {
			body: { query }
		});
		if (!data) {
			return fail(response.status, {
				query,
				message: problemMessage(problem, 'The query failed.')
			});
		}
		return { query, result: data };
	}
};
