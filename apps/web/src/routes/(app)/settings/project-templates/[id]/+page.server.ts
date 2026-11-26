import { error, fail, redirect } from '@sveltejs/kit';
import { homeOrg } from '#lib/dashboards/server.ts';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	if (event.params.id === 'new') return { template: null };
	const { data } = await api(event).GET('/api/v1/project-templates/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (!data) error(404, 'There’s no such template.');
	return { template: data };
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		let structure: unknown;
		try {
			structure = JSON.parse(formString(form, 'structure'));
		} catch {
			return fail(422, { message: 'The template couldn’t be read.' });
		}
		const body = {
			orgId: formString(form, 'orgId') || ((await homeOrg(event)) ?? ''),
			name: formString(form, 'name'),
			description: formString(form, 'description'),
			structure: structure as never,
			version: formString(form, 'version') ? Number(formString(form, 'version')) : null
		};
		const result =
			event.params.id === 'new'
				? await api(event).POST('/api/v1/project-templates', { body })
				: await api(event).PUT('/api/v1/project-templates/{id}', {
						params: { path: { id: event.params.id } },
						body
					});
		if (!result.data) {
			return fail(result.response.status, {
				message: problemMessage(result.error, 'Could not save the template.')
			});
		}
		redirect(303, '/settings/project-templates');
	},
	remove: async (event) => {
		await api(event).DELETE('/api/v1/project-templates/{id}', {
			params: { path: { id: event.params.id } }
		});
		redirect(303, '/settings/project-templates');
	}
};
