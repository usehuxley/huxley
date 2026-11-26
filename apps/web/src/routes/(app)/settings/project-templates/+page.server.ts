import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/project-templates');
	if (response.status === 404) error(404, 'Projects are turned off.');
	return { templates: data ?? [] };
};
