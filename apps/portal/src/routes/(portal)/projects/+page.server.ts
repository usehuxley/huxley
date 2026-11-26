import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/portal/v1/projects');
	if (!data) error(response.status === 403 ? 404 : 503, 'Projects aren’t available.');
	return { projects: data };
};
