import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const status = (['draft', 'approved', 'sent', 'void'] as const).find(
		(s) => s === params.get('status')
	);
	const { data, response } = await api(event).GET('/api/v1/invoices', {
		params: {
			query: { status, runId: params.get('runId') || undefined, q: params.get('q') || undefined }
		}
	});
	if (response.status === 404) error(404, 'Invoicing is turned off.');
	if (!data) error(response.status === 403 ? 403 : 503, 'Could not load invoices.');
	const { user } = await event.parent();
	return {
		invoices: data,
		status: status ?? '',
		q: params.get('q') ?? '',
		runId: params.get('runId') ?? '',
		canApprove: user.grantedPermissions.includes('billing:approve')
	};
};

export const actions: Actions = {
	approveRun: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/billing/runs/{id}/approve', {
			params: { path: { id: formString(form, 'runId') } },
			body: { submitReviews: form.get('submitReviews') === 'on' }
		});
		if (!data)
			return fail(response.status, {
				message: problemMessage(problem, 'Could not approve the run.')
			});
		return { approval: data };
	}
};
