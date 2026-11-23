import { error, fail } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { readFieldValues } from '#lib/fields/values.ts';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const path = { params: { path: { entity_type: 'organization', id } } };
	const [org, fields, tags, allTags] = await Promise.all([
		client.GET('/api/v1/orgs/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/entities/{entity_type}/{id}/fields', path),
		client.GET('/api/v1/entities/{entity_type}/{id}/tags', path),
		client.GET('/api/v1/tags')
	]);
	if (org.response.status === 404) error(404, 'Organization not found.');
	if (!org.data || !fields.data || !tags.data) error(503, 'Could not load the organization.');
	return { org: org.data, fields: fields.data, tags: tags.data, allTags: allTags.data ?? [] };
};

export const actions: Actions = {
	fields: async (event) => {
		const form = await event.request.formData();
		const path = { entity_type: 'organization', id: event.params.id };
		const current = await api(event).GET('/api/v1/entities/{entity_type}/{id}/fields', {
			params: { path }
		});
		const values = readFieldValues(form, current.data ?? []);
		const { error: problem, response } = await api(event).PATCH(
			'/api/v1/entities/{entity_type}/{id}/fields',
			{ params: { path }, body: { values } }
		);
		if (response.status !== 200) {
			const errors: Record<string, string> = {};
			if (isApiProblem(problem)) {
				for (const e of problem.errors ?? []) errors[e.pointer.replace(/^\//, '')] = e.message;
			}
			return fail(response.status, {
				fieldErrors: errors,
				message: problemMessage(problem, 'Could not save the fields.')
			});
		}
		return { message: 'Saved.' };
	},

	tag: async (event) => {
		const form = await event.request.formData();
		const path = {
			entity_type: 'organization',
			id: event.params.id,
			tag_id: formString(form, 'tagId')
		};
		const remove = formString(form, 'remove') === 'true';
		const { response } = remove
			? await api(event).DELETE('/api/v1/entities/{entity_type}/{id}/tags/{tag_id}', {
					params: { path }
				})
			: await api(event).PUT('/api/v1/entities/{entity_type}/{id}/tags/{tag_id}', {
					params: { path }
				});
		if (response.status !== 204)
			return fail(response.status, { message: 'Could not change the tag.' });
		return { message: remove ? 'Tag removed.' : 'Tag added.' };
	}
};
