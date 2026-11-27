import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type Status = components['schemas']['PurchaseOrderStatus'];
const STATUSES: Status[] = [
	'draft',
	'submitted',
	'approved',
	'ordered',
	'partially_received',
	'received',
	'cancelled'
];

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const status = STATUSES.find((s) => s === params.get('status'));
	const client = api(event);
	const { user } = await event.parent();
	const canWrite = user.grantedPermissions.includes('procurement:write');
	const [orders, vendors, orgs] = await Promise.all([
		client.GET('/api/v1/purchase-orders', {
			params: { query: { status, q: params.get('q') || undefined } }
		}),
		canWrite ? client.GET('/api/v1/vendors') : null,
		canWrite ? client.GET('/api/v1/orgs') : null
	]);
	if (orders.response.status === 404) error(404, 'Purchasing is turned off.');
	if (!orders.data) error(orders.response.status === 403 ? 403 : 503, 'Could not load orders.');
	const providers = (orgs?.data ?? []).filter((o) => o.kind === 'provider');
	return {
		orders: orders.data,
		vendors: (vendors?.data ?? []).map((v) => ({ value: v.id, label: v.name, orgId: v.orgId })),
		providers: providers.map((o) => ({ value: o.id, label: o.name })),
		status: status ?? '',
		q: params.get('q') ?? '',
		canWrite
	};
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/purchase-orders', {
			body: { orgId: formString(form, 'orgId'), order: { vendorId: formString(form, 'vendorId') } }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not start it.') });
		}
		redirect(303, `/purchasing/${data.id}`);
	},
	reorder: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/purchase-orders/reorder', {
			body: { orgId: formString(form, 'orgId') }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not reorder.') });
		}
		return {
			message:
				data.length === 0
					? 'Nothing to reorder (or no vendor matches the low items).'
					: `Drafted ${data.length} order${data.length === 1 ? '' : 's'}.`
		};
	}
};
