import { error, fail, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { formString } from '#lib/forms.ts';
import { homeOrg } from '#lib/dashboards/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/dashboards');
	if (!data) error(response.status, 'You can’t see dashboards.');
	const { user } = await event.parent();
	const waiting = user.grantedPermissions.includes('dashboard:manage')
		? ((await api(event).GET('/api/v1/dashboard-submissions')).data ?? []).filter(
				(s) => s.submittedBy !== user.id
			)
		: [];
	return { dashboards: data, waiting };
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const name = formString(form, 'name') || 'My dashboard';
		const orgId = await homeOrg(event);
		if (!orgId) return fail(400, { message: 'You need access to an organization.' });
		const { data, response } = await api(event).POST('/api/v1/dashboards', {
			body: { orgId, shared: false, dashboard: { name, widgets: [] } }
		});
		if (!data) return fail(response.status, { message: 'Could not create it.' });
		redirect(303, `/dashboards/${data.id}/edit`);
	}
};
