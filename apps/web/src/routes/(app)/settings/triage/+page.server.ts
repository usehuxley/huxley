import { error, fail } from '@sveltejs/kit';
import { homeOrg } from '#lib/dashboards/server.ts';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const orgId = await homeOrg(event);
	if (!orgId) error(403, 'You need access to an organization.');
	const query = { params: { query: { orgId } } };
	const client = api(event);
	const [samples, evaluations, agreement, queues] = await Promise.all([
		client.GET('/api/v1/triage/samples', query),
		client.GET('/api/v1/triage/evaluations', query),
		client.GET('/api/v1/triage/agreement', query),
		client.GET('/api/v1/triage/options', query)
	]);
	if (samples.response.status === 404) error(404, 'The service desk is turned off.');
	if (!samples.data) error(samples.response.status, 'Could not load AI triage.');
	return {
		orgId,
		samples: samples.data,
		evaluations: evaluations.data ?? [],
		agreement: agreement.data ?? null,
		queues: queues.data ?? []
	};
};

export const actions: Actions = {
	add: async (event) => {
		const form = await event.request.formData();
		const orgId = await homeOrg(event);
		const { error: problem, response } = await api(event).POST('/api/v1/triage/samples', {
			body: {
				orgId: orgId ?? '',
				text: formString(form, 'text'),
				queueId: formString(form, 'queueId') || null,
				priority: formString(form, 'priority') || null
			}
		});
		if (response.status !== 201) {
			return fail(response.status, { sample: problemMessage(problem, 'Could not add it.') });
		}
		return { sample: 'Sample added.' };
	},
	fromTickets: async (event) => {
		const orgId = await homeOrg(event);
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/triage/samples/from-tickets', {
			body: { orgId: orgId ?? '', limit: 50 }
		});
		if (!data) {
			return fail(response.status, { sample: problemMessage(problem, 'Could not add them.') });
		}
		return { sample: `Added ${data.added} resolved ticket${data.added === 1 ? '' : 's'}.` };
	},
	remove: async (event) => {
		const form = await event.request.formData();
		await api(event).DELETE('/api/v1/triage/samples/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		return { sample: 'Sample removed.' };
	},
	evaluate: async (event) => {
		const orgId = await homeOrg(event);
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/triage/evaluations', {
			body: { orgId: orgId ?? '' }
		});
		if (!data) {
			return fail(response.status, { evaluate: problemMessage(problem, 'Could not start it.') });
		}
		return { evaluate: `Running ${data.total} samples through the workflow.` };
	}
};
