import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { completeActivity } from '#lib/sales/activities.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [open, recent] = await Promise.all([
		client.GET('/api/v1/activities', { params: { query: { mine: true, open: true } } }),
		client.GET('/api/v1/activities', { params: { query: { mine: true, limit: 30 } } })
	]);
	if (open.response.status === 404) error(404, 'Sales is turned off.');
	if (!open.data) error(503, 'Could not load activities.');
	const { user } = await event.parent();
	return {
		open: open.data,
		recent: (recent.data ?? []).filter((a) => a.doneAt),
		canWrite: user.grantedPermissions.includes('sales:write')
	};
};

export const actions: Actions = { completeActivity: (event) => completeActivity(event) };
