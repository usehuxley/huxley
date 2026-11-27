import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors, opportunityBody } from '#lib/sales/server.ts';
import { assignees } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [found, orgs, people] = await Promise.all([
		client.GET('/api/v1/leads/{id}', { params: { path: { id: event.params.id } } }),
		client.GET('/api/v1/orgs'),
		assignees(event)
	]);
	if (found.response.status === 404) error(404, 'Lead not found.');
	if (!found.data) error(503, 'Could not load the lead.');
	const { user } = await event.parent();
	return {
		lead: found.data,
		clients: (orgs.data ?? [])
			.filter((o) => o.kind === 'client' && o.status === 'active')
			.map((o) => ({ value: o.id, label: o.name })),
		people: people.people,
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
		} = await api(event).PUT('/api/v1/leads/{id}', {
			params: { path: { id: event.params.id } },
			body: {
				companyName: formString(form, 'companyName'),
				firstName: formString(form, 'firstName') || null,
				lastName: formString(form, 'lastName') || null,
				email: formString(form, 'email') || null,
				phone: formString(form, 'phone') || null,
				source: formString(form, 'source') || null,
				notes: formString(form, 'notes'),
				ownerId: formString(form, 'ownerId') || null,
				status: (formString(form, 'status') || null) as 'new' | 'working' | null,
				version: Number(formString(form, 'version'))
			}
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
	disqualify: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/leads/{id}/disqualify', {
			params: { path: { id: event.params.id } },
			body: { reason: formString(form, 'reason'), version: Number(formString(form, 'version')) }
		});
		if (!data) {
			return fail(response.status, {
				disqualify: { message: problemMessage(problem, 'Could not disqualify it.') }
			});
		}
		return { disqualified: true };
	},
	convert: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/leads/{id}/convert', {
			params: { path: { id: event.params.id } },
			body: {
				orgId: formString(form, 'clientId') || null,
				createContact: formString(form, 'createContact') === 'on',
				opportunity: opportunityBody(form),
				version: Number(formString(form, 'version'))
			}
		});
		if (!data) {
			return fail(response.status, {
				convert: {
					message: problemMessage(problem, 'Could not convert it.'),
					errors: fieldErrors(problem)
				}
			});
		}
		redirect(303, `/sales/opportunities/${data.opportunity.id}`);
	},
	delete: async (event) => {
		const { response, error: problem } = await api(event).DELETE('/api/v1/leads/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (!response.ok) {
			return fail(response.status, {
				save: { message: problemMessage(problem, 'Could not delete it.'), errors: {} }
			});
		}
		redirect(303, '/sales/leads');
	}
};
