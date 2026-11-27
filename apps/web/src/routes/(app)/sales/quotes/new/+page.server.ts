import { fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

/** Starts a quote: the client and a title (from an opportunity when given). */
export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const opportunityId = event.url.searchParams.get('opportunity');
	const [orgs, opportunity] = await Promise.all([
		client.GET('/api/v1/orgs'),
		opportunityId
			? client.GET('/api/v1/opportunities/{id}', { params: { path: { id: opportunityId } } })
			: null
	]);
	return {
		clients: (orgs.data ?? [])
			.filter((o) => o.kind === 'client' && o.status === 'active')
			.map((o) => ({ value: o.id, label: o.name })),
		opportunity: opportunity?.data ?? null,
		orgId: opportunity?.data?.orgId ?? event.url.searchParams.get('org') ?? ''
	};
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/quotes', {
			body: {
				orgId: formString(form, 'orgId'),
				quote: {
					title: formString(form, 'title'),
					opportunityId: formString(form, 'opportunityId') || null,
					currency: formString(form, 'currency') || 'USD',
					contactId: formString(form, 'contactId') || null
				}
			}
		});
		if (!data)
			return fail(response.status, {
				message: problemMessage(problem, 'Could not start the quote.')
			});
		redirect(303, `/sales/quotes/${data.id}`);
	}
};
