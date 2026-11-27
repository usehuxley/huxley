import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors, opportunityBody } from '#lib/sales/server.ts';
import { assignees } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [found, history, stages, reasons, people] = await Promise.all([
		client.GET('/api/v1/opportunities/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/opportunities/{id}/history', { params: { path: { id } } }),
		client.GET('/api/v1/sales/stages'),
		client.GET('/api/v1/sales/reasons'),
		assignees(event)
	]);
	if (found.response.status === 404) error(404, 'Opportunity not found.');
	if (!found.data) error(503, 'Could not load the opportunity.');
	const contacts = await client.GET('/api/v1/orgs/{id}/contacts', {
		params: { path: { id: found.data.orgId }, query: { limit: 200 } }
	});
	const { user } = await event.parent();
	return {
		opportunity: found.data,
		history: history.data ?? [],
		stages: stages.data ?? [],
		reasons: reasons.data ?? [],
		people: people.people,
		contacts: (contacts.data?.items ?? []).map((c) => ({ value: c.id, label: c.name })),
		canWrite: user.grantedPermissions.includes('sales:write')
	};
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/opportunities/{id}', {
			params: { path: { id: event.params.id } },
			body: { ...opportunityBody(form), version: Number(formString(form, 'version')) }
		});
		if (!data) {
			return fail(response.status, {
				save: {
					message: problemMessage(problem, 'Could not save it.'),
					errors: fieldErrors(problem)
				}
			});
		}
		return { saved: true };
	},
	move: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/opportunities/{id}/move', {
			params: { path: { id: event.params.id } },
			body: {
				stageId: formString(form, 'stageId'),
				reasonId: formString(form, 'reasonId') || null,
				reasonNote: formString(form, 'reasonNote') || null,
				version: Number(formString(form, 'version'))
			}
		});
		if (!data) {
			return fail(response.status, {
				move: { message: problemMessage(problem, 'Could not move it.') }
			});
		}
		return { moved: data.stageName };
	},
	delete: async (event) => {
		const { response, error: problem } = await api(event).DELETE('/api/v1/opportunities/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (!response.ok) {
			return fail(response.status, {
				move: { message: problemMessage(problem, 'Could not delete it.') }
			});
		}
		redirect(303, '/sales');
	}
};
