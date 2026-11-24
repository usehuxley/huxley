import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/canned-responses');
	if (response.status === 404) error(404, 'The service desk is turned off.');
	if (!data) error(503, 'Could not load the responses.');
	return { responses: data };
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { errors: fieldErrors(problem), message: problemMessage(problem, fallback) });

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const version = formString(form, 'version');
		const body = {
			name: formString(form, 'name'),
			category: formString(form, 'category') || null,
			kind: formString(form, 'kind') || 'public',
			body: formString(form, 'body'),
			...(version ? { version: Number(version) } : {})
		};
		const { error: problem, response } = id
			? await api(event).PUT('/api/v1/canned-responses/{id}', { params: { path: { id } }, body })
			: await api(event).POST('/api/v1/canned-responses', { body });
		if (response.status !== 200 && response.status !== 201)
			return failed(response.status, problem, 'Could not save the response.');
		return { saved: true };
	},
	archive: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/canned-responses/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) return failed(response.status, problem, 'Could not archive it.');
		return { saved: true };
	}
};
