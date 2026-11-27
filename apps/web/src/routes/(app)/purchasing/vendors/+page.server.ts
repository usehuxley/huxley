import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [vendors, orgs] = await Promise.all([
		client.GET('/api/v1/vendors', { params: { query: { all: true } } }),
		client.GET('/api/v1/orgs')
	]);
	if (vendors.response.status === 404) error(404, 'Purchasing is turned off.');
	if (!vendors.data) error(vendors.response.status === 403 ? 403 : 503, 'Could not load vendors.');
	const { user } = await event.parent();
	return {
		vendors: vendors.data,
		providers: (orgs.data ?? [])
			.filter((o) => o.kind === 'provider')
			.map((o) => ({ value: o.id, label: o.name })),
		canWrite: user.grantedPermissions.includes('procurement:write')
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const opt = (k: string) => formString(form, k).trim() || null;
		const vendor = {
			name: formString(form, 'name'),
			accountNumber: opt('accountNumber'),
			email: opt('email'),
			phone: opt('phone'),
			website: opt('website'),
			address: opt('address'),
			currency: formString(form, 'currency') || 'USD',
			paymentTerms: opt('paymentTerms'),
			notes: opt('notes'),
			active: form.get('active') === 'on'
		};
		const client = api(event);
		const {
			data,
			error: problem,
			response
		} = id
			? await client.PUT('/api/v1/vendors/{id}', { params: { path: { id } }, body: vendor })
			: await client.POST('/api/v1/vendors', {
					body: { orgId: formString(form, 'orgId'), vendor }
				});
		if (!data) return failed(response.status, problem, 'Could not save the vendor.');
		return { saved: data.name };
	},
	delete: async (event) => {
		const form = await event.request.formData();
		const { response, error: problem } = await api(event).DELETE('/api/v1/vendors/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (!response.ok) return failed(response.status, problem, 'Could not remove it.');
		return { removed: true };
	}
};
