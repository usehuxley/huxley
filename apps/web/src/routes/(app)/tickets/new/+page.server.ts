import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { assignees, fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const orgId = event.url.searchParams.get('org') ?? '';
	const templateId = event.url.searchParams.get('template') ?? '';
	const [companies, types, queues, people, contacts, templates] = await Promise.all([
		client.GET('/api/v1/companies', { params: { query: { providers: true } } }),
		client.GET('/api/v1/servicedesk/types'),
		client.GET('/api/v1/queues'),
		assignees(event),
		orgId
			? client.GET('/api/v1/orgs/{id}/contacts', {
					params: { path: { id: orgId }, query: { limit: 200 } }
				})
			: null,
		client.GET('/api/v1/ticket-templates')
	]);
	if (types.response.status === 404) error(404, 'The service desk is turned off.');
	if (!companies.data || !types.data) error(503, 'Could not load the form.');
	return {
		companies: companies.data,
		types: types.data,
		queues: queues.data ?? [],
		...people,
		contacts: contacts?.data?.items ?? [],
		orgId,
		templates: templates.data ?? [],
		template: (templates.data ?? []).find((t) => t.id === templateId) ?? null
	};
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const optional = (key: string) => formString(form, key) || undefined;
		const assignee = formString(form, 'assignee');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/tickets', {
			body: {
				orgId: formString(form, 'orgId'),
				type: optional('type'),
				title: formString(form, 'title'),
				description: optional('description'),
				impact: Number(formString(form, 'impact')) || undefined,
				urgency: Number(formString(form, 'urgency')) || undefined,
				queueId: optional('queueId'),
				requesterId: optional('requesterId'),
				assigneeUserId: assignee.startsWith('user:') ? assignee.slice(5) : undefined,
				assigneeTeamId: assignee.startsWith('team:') ? assignee.slice(5) : undefined,
				source: optional('source')
			}
		});
		if (response.status !== 201 || !data) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not create the ticket.')
			});
		}
		redirect(303, `/tickets/${data.id}`);
	}
};
