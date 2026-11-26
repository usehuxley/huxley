import { error, fail, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { formString } from '#lib/forms.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const [{ data: dashboard }, { data: catalog }] = await Promise.all([
		api(event).GET('/api/v1/dashboards/{id}', { params: { path: { id: event.params.id } } }),
		api(event).GET('/api/v1/analytics/sources')
	]);
	if (!dashboard || dashboard.builtin) error(404, 'There’s no such dashboard to change.');
	const { user } = await event.parent();
	return {
		dashboard,
		sources: catalog?.sources ?? [],
		canShare: user.grantedPermissions.includes('dashboard:manage')
	};
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		let body: components['schemas']['DashboardInput'];
		try {
			body = JSON.parse(formString(form, 'dashboard')) as components['schemas']['DashboardInput'];
		} catch {
			return fail(400, { message: 'The dashboard could not be read.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/dashboards/{id}', {
			params: { path: { id: event.params.id } },
			body
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not save it.') });
		}
		redirect(303, `/dashboards/${event.params.id}`);
	},
	delete: async (event) => {
		const { response, error: problem } = await api(event).DELETE('/api/v1/dashboards/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (response.status !== 204) {
			return fail(response.status, { message: problemMessage(problem, 'Could not delete it.') });
		}
		redirect(303, '/dashboards');
	}
};
