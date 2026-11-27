import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/expense-reports');
	if (response.status === 404) error(404, 'Expenses are turned off.');
	if (!data) error(503, 'Could not load expense reports.');
	const { user } = await event.parent();
	return { reports: data, me: user.id };
};
