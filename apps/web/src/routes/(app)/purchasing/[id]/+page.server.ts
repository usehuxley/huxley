import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { serialList } from '#lib/inventory/labels.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type OrderInput = components['schemas']['PurchaseOrderInput'];
type Step = components['schemas']['PurchaseOrderStep'];
type ReceiptLine = components['schemas']['PurchaseOrderReceiptLine'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [order, receipts, items, locations, vendors] = await Promise.all([
		client.GET('/api/v1/purchase-orders/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/purchase-orders/{id}/receipts', { params: { path: { id } } }),
		client.GET('/api/v1/inventory/items'),
		client.GET('/api/v1/inventory/locations'),
		client.GET('/api/v1/vendors')
	]);
	if (order.response.status === 404) error(404, 'Purchase order not found.');
	if (!order.data) error(503, 'Could not load the order.');
	const o = order.data;
	const { user } = await event.parent();
	return {
		order: o,
		receipts: receipts.data ?? [],
		items: (items.data ?? [])
			.filter((i) => i.orgId === o.orgId)
			.map((i) => ({ id: i.id, sku: i.sku, name: i.name, tracking: i.tracking })),
		locations: (locations.data ?? [])
			.filter((l) => l.orgId === o.orgId)
			.map((l) => ({ value: l.id, label: l.parentName ? `${l.parentName} › ${l.name}` : l.name })),
		vendors: (vendors.data ?? [])
			.filter((v) => v.orgId === o.orgId)
			.map((v) => ({ value: v.id, label: v.name })),
		canWrite: user.grantedPermissions.includes('procurement:write'),
		canApprove: user.grantedPermissions.includes('procurement:approve')
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		let doc: OrderInput;
		try {
			doc = JSON.parse(formString(form, 'doc')) as OrderInput;
		} catch {
			return fail(400, { message: 'Could not read the order.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/purchase-orders/{id}', {
			params: { path: { id: event.params.id } },
			body: doc
		});
		if (!data) return failed(response.status, problem, 'Could not save the order.');
		return { message: 'Saved.' };
	},
	step: async (event) => {
		const form = await event.request.formData();
		const step = formString(form, 'step') as Step;
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/purchase-orders/{id}/step', {
			params: { path: { id: event.params.id } },
			body: {
				step,
				version: Number(formString(form, 'version')),
				comment: formString(form, 'comment') || null
			}
		});
		if (!data) return failed(response.status, problem, 'Could not do that.');
		const said: Record<Step, string> = {
			submit: data.status === 'submitted' ? 'Submitted for approval.' : 'Approved.',
			approve: 'Approved.',
			reject: 'Sent back.',
			order: 'Marked ordered.',
			cancel: 'Cancelled.',
			close: 'Closed.'
		};
		return { message: said[step] };
	},
	receive: async (event) => {
		const form = await event.request.formData();
		const lines: ReceiptLine[] = [];
		for (const lineId of form.getAll('lineId').map(String)) {
			const quantity = formString(form, `quantity-${lineId}`).trim();
			const serials = serialList(formString(form, `serials-${lineId}`));
			if (!serials.length && (!quantity || Number(quantity) === 0)) continue;
			lines.push({
				lineId,
				quantity: serials.length ? null : quantity,
				serials,
				lot: formString(form, `lot-${lineId}`) || null
			});
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/purchase-orders/{id}/receive', {
			params: { path: { id: event.params.id } },
			body: {
				locationId: formString(form, 'locationId') || null,
				reference: formString(form, 'reference') || null,
				lines
			}
		});
		if (!data) return failed(response.status, problem, 'Could not receive it.');
		return { message: data.status === 'received' ? 'All received.' : 'Received in part.' };
	}
};
