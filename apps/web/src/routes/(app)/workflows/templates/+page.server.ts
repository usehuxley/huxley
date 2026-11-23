import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/v1/templates');
	if (!data) error(503, 'Could not load templates.');
	return { templates: data };
};
