import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const view = event.url.searchParams.get('view') === 'matrix' ? 'matrix' : 'people';
	const { user } = await event.parent();
	const manager = user.grantedPermissions.includes('workforce:manage');
	const [people, skills, matrix, waiting] = await Promise.all([
		client.GET('/api/v1/workforce/people'),
		client.GET('/api/v1/workforce/skills'),
		view === 'matrix' ? client.GET('/api/v1/workforce/matrix') : null,
		manager
			? client.GET('/api/v1/workforce/time-off', { params: { query: { status: 'requested' } } })
			: null
	]);
	if (people.response.status === 404) error(404, 'Workforce is turned off.');
	if (!people.data || !skills.data) error(503, 'Could not load people.');
	return {
		people: people.data,
		skills: skills.data,
		matrix: matrix?.data ?? [],
		waiting: waiting?.data ?? [],
		view
	};
};
