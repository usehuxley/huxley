import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { lastMonth } from '#lib/billing/labels.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

const day = /^\d{4}-\d{2}-\d{2}$/;

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const client = api(event);
	const orgs = await client.GET('/api/v1/orgs');
	const providers = (orgs.data ?? []).filter((o) => o.kind === 'provider');
	const orgId = params.get('org') || providers[0]?.id || '';
	const fallback = lastMonth();
	const from = day.test(params.get('from') ?? '') ? params.get('from')! : fallback.from;
	const to = day.test(params.get('to') ?? '') ? params.get('to')! : fallback.to;
	const [unbilled, runs, queue] = await Promise.all([
		orgId
			? client.GET('/api/v1/billing/unbilled', { params: { query: { orgId, from, to } } })
			: null,
		client.GET('/api/v1/billing/runs'),
		client.GET('/api/v1/billing/sync-queue', { params: { query: { status: 'failed' } } })
	]);
	if (unbilled?.response.status === 404) error(404, 'Invoicing is turned off.');
	if (unbilled && !unbilled.data)
		error(unbilled.response.status === 403 ? 403 : 503, 'Could not load unbilled work.');
	const { user } = await event.parent();
	return {
		unbilled: unbilled?.data ?? [],
		runs: runs.data ?? [],
		syncErrors: queue.data?.length ?? 0,
		providers: providers.map((o) => ({ value: o.id, label: o.name })),
		orgId,
		from,
		to,
		canWrite: user.grantedPermissions.includes('billing:write')
	};
};

export const actions: Actions = {
	run: async (event) => {
		const form = await event.request.formData();
		const clients = form.getAll('clients').map(String);
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/billing/runs', {
			body: {
				orgId: formString(form, 'orgId'),
				from: formString(form, 'from'),
				to: formString(form, 'to'),
				clients: clients.length ? clients : null
			}
		});
		if (!data)
			return fail(response.status, { message: problemMessage(problem, 'Could not run billing.') });
		redirect(303, `/billing/invoices?runId=${data.id}`);
	}
};
