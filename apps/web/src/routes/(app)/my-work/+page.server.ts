import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/my-work');
	if (!data) error(response.status === 404 ? 404 : response.status, 'Could not load your work.');
	return { work: data };
};
