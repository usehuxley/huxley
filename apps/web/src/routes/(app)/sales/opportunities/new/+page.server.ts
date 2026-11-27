import { fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors, opportunityBody } from '#lib/sales/server.ts';
import { assignees } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [orgs, stages, people] = await Promise.all([
		client.GET('/api/v1/orgs'),
		client.GET('/api/v1/sales/stages'),
		assignees(event)
	]);
	return {
		clients: (orgs.data ?? [])
			.filter((o) => o.kind === 'client' && o.status === 'active')
			.map((o) => ({ value: o.id, label: o.name })),
		stages: (stages.data ?? [])
			.filter((s) => s.kind === 'open')
			.map((s) => ({ value: s.id, label: `${s.name} (${s.probability}%)` })),
		people: people.people,
		orgId: event.url.searchParams.get('org') ?? ''
	};
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/opportunities', {
			body: {
				orgId: formString(form, 'orgId'),
				opportunity: { ...opportunityBody(form), stageId: formString(form, 'stageId') || null }
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create the opportunity.'),
				errors: fieldErrors(problem)
			});
		}
		redirect(303, `/sales/opportunities/${data.id}`);
	}
};
