import { error, fail, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { answersFor, homeOrg } from '#lib/dashboards/server.ts';
import { RANGES } from '#lib/dashboards/spec.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data: dashboard, response } = await api(event).GET('/api/v1/dashboards/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (!dashboard) error(response.status === 403 ? 403 : 404, 'There’s no such dashboard.');
	const asked = event.url.searchParams.get('range') ?? '';
	const range = RANGES.some((r) => r.value === asked) ? asked : dashboard.range;
	const orgId = event.url.searchParams.get('org') || null;
	const [{ results }, orgs] = await Promise.all([
		answersFor(event, dashboard, range, orgId),
		api(event).GET('/api/v1/orgs')
	]);
	const { user } = await event.parent();
	const canEdit =
		!dashboard.builtin &&
		(dashboard.ownerUserId === user.id || user.grantedPermissions.includes('dashboard:manage'));
	return {
		dashboard,
		range,
		orgId,
		results,
		orgs: (orgs.data ?? []).map((o) => ({ value: o.id, label: o.name })),
		canEdit
	};
};

export const actions: Actions = {
	copy: async (event) => {
		const { data: dashboard } = await api(event).GET('/api/v1/dashboards/{id}', {
			params: { path: { id: event.params.id } }
		});
		const orgId = await homeOrg(event);
		if (!dashboard || !orgId) return fail(404, { message: 'Could not copy it.' });
		const { data, response } = await api(event).POST('/api/v1/dashboards', {
			body: {
				orgId,
				shared: false,
				dashboard: {
					name: `${dashboard.name} (copy)`,
					description: dashboard.description,
					range: dashboard.range,
					widgets: dashboard.widgets
				}
			}
		});
		if (!data) return fail(response.status, { message: 'Could not copy it.' });
		redirect(303, `/dashboards/${data.id}/edit`);
	}
};
