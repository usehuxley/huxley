import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { articleFields } from '#lib/kb/form.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [article, categories] = await Promise.all([
		client.GET('/api/v1/kb/articles/{id}', { params: { path: { id: event.params.id } } }),
		client.GET('/api/v1/kb/categories')
	]);
	if (!article.data)
		error(article.response.status === 404 ? 404 : 503, 'Could not load the article.');
	if (!article.data.canWrite) error(403, 'You may not change this article.');
	return {
		article: article.data,
		categories: (categories.data ?? []).map((c) => ({ value: c.id, label: c.name }))
	};
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).PATCH('/api/v1/kb/articles/{id}', {
			params: { path: { id: event.params.id } },
			body: { version: Number(formString(form, 'version')), ...articleFields(form) }
		});
		if (!data) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not save the article.')
			});
		}
		redirect(303, `/knowledge/${data.id}`);
	}
};
