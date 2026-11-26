import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const asked = event.url.searchParams.get('view');
	const { user } = await event.parent();
	const manager = user.grantedPermissions.includes('workforce:manage');
	const view = asked === 'matrix' ? 'matrix' : asked === 'gaps' && manager ? 'gaps' : 'people';
	const [people, skills, matrix, waiting, gaps] = await Promise.all([
		client.GET('/api/v1/workforce/people'),
		client.GET('/api/v1/workforce/skills'),
		view === 'matrix' ? client.GET('/api/v1/workforce/matrix') : null,
		manager
			? client.GET('/api/v1/workforce/time-off', { params: { query: { status: 'requested' } } })
			: null,
		view === 'gaps' ? client.GET('/api/v1/workforce/insights') : null
	]);
	if (people.response.status === 404) error(404, 'Workforce is turned off.');
	if (!people.data || !skills.data) error(503, 'Could not load people.');
	return {
		people: people.data,
		skills: skills.data,
		matrix: matrix?.data ?? [],
		waiting: waiting?.data ?? [],
		gaps: gaps?.data ?? null,
		manager,
		view
	};
};
