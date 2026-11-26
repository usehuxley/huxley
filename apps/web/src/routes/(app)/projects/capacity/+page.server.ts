import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const from = event.url.searchParams.get('from') ?? undefined;
	const teamId = event.url.searchParams.get('team') || undefined;
	const [plan, teams] = await Promise.all([
		client.GET('/api/v1/projects/capacity', { params: { query: { from, weeks: 6, teamId } } }),
		client.GET('/api/v1/teams')
	]);
	if (plan.response.status === 404) error(404, 'Projects are turned off.');
	if (!plan.data) error(plan.response.status, 'Could not load capacity.');
	return {
		plan: plan.data,
		teams: (teams.data ?? []).map((t) => ({ value: t.id, label: t.name })),
		team: teamId ?? ''
	};
};
