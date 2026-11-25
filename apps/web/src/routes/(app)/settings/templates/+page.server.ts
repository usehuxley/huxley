import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [templates, types, queues] = await Promise.all([
		client.GET('/api/v1/ticket-templates'),
		client.GET('/api/v1/servicedesk/types'),
		client.GET('/api/v1/queues')
	]);
	if (templates.response.status === 404) error(404, 'The service desk is turned off.');
	if (!templates.data) error(503, 'Could not load the templates.');
	return { templates: templates.data, types: types.data ?? [], queues: queues.data ?? [] };
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
			typeKey: formString(form, 'typeKey') || null,
			title: formString(form, 'title') || null,
			description: formString(form, 'description') || null,
			impact: Number(formString(form, 'impact') || 2),
			urgency: Number(formString(form, 'urgency') || 2),
			queueId: formString(form, 'queueId') || null,
			...(version ? { version: Number(version) } : {})
		};
		const { error: problem, response } = id
			? await api(event).PUT('/api/v1/ticket-templates/{id}', { params: { path: { id } }, body })
			: await api(event).POST('/api/v1/ticket-templates', { body });
		if (response.status !== 200 && response.status !== 201)
			return failed(response.status, problem, 'Could not save the template.');
		return { saved: true };
	},
	archive: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/ticket-templates/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) return failed(response.status, problem, 'Could not archive it.');
		return { saved: true };
	}
};
