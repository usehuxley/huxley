import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/sales/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const status = (['new', 'working', 'disqualified', 'converted'] as const).find(
		(s) => s === params.get('status')
	);
	const client = api(event);
	const [leads, orgs] = await Promise.all([
		client.GET('/api/v1/leads', {
			params: { query: { status: status, q: params.get('q') || undefined } }
		}),
		client.GET('/api/v1/orgs')
	]);
	if (leads.response.status === 404) error(404, 'Sales is turned off.');
	if (!leads.data) error(503, 'Could not load leads.');
	const { user } = await event.parent();
	return {
		leads: leads.data,
		status: status ?? '',
		q: params.get('q') ?? '',
		providers: (orgs.data ?? [])
			.filter((o) => o.kind === 'provider' && o.status === 'active')
			.map((o) => ({ value: o.id, label: o.name })),
		canWrite: user.grantedPermissions.includes('sales:write')
	};
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/leads', {
			body: {
				orgId: formString(form, 'orgId'),
				lead: {
					companyName: formString(form, 'companyName'),
					firstName: formString(form, 'firstName') || null,
					lastName: formString(form, 'lastName') || null,
					email: formString(form, 'email') || null,
					phone: formString(form, 'phone') || null,
					source: formString(form, 'source') || null,
					notes: formString(form, 'notes')
				}
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not add the lead.'),
				errors: fieldErrors(problem)
			});
		}
		redirect(303, `/sales/leads/${data.id}`);
	}
};
