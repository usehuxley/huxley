import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const orgs = await client.GET('/api/v1/orgs');
	const providers = (orgs.data ?? []).filter((o) => o.kind === 'provider');
	const all = await Promise.all(
		providers.map(async (o) => {
			const { data, response } = await client.GET('/api/v1/billing/settings', {
				params: { query: { orgId: o.id } }
			});
			if (response.status === 404) error(404, 'Invoicing is turned off.');
			return { orgId: o.id, name: o.name, settings: data };
		})
	);
	return { providers: all };
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const rate = formString(form, 'hourlyRate').trim();
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/billing/settings', {
			body: {
				orgId: formString(form, 'orgId'),
				hourlyRate: rate || null,
				currency: formString(form, 'currency') || 'USD',
				paymentDays: Number(formString(form, 'paymentDays') || '30'),
				approvedTimeOnly: form.get('approvedTimeOnly') === 'on'
			}
		});
		if (!data)
			return fail(response.status, { message: problemMessage(problem, 'Could not save.') });
		return { message: 'Saved.' };
	}
};
