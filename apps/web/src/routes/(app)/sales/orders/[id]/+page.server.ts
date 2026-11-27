import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const { user } = await event.parent();
	const can = (p: string) => user.grantedPermissions.includes(p);
	const on = (m: string) => user.activeModules.includes(m);
	const [order, templates] = await Promise.all([
		client.GET('/api/v1/sales-orders/{id}', { params: { path: { id } } }),
		on('projects') && can('project:manage') ? client.GET('/api/v1/project-templates') : null
	]);
	if (order.response.status === 404) error(404, 'Order not found.');
	if (!order.data) error(503, 'Could not load the order.');
	return {
		order: order.data,
		templates: (templates?.data ?? []).map((t) => ({ value: t.id, label: t.name })),
		can: {
			write: can('sales:write'),
			stock: on('inventory') && can('inventory:write'),
			buy: on('inventory') && can('procurement:write'),
			agreement: on('contracts') && can('agreement:manage'),
			project: on('projects') && can('project:manage')
		}
	};
};

type Said = { message: string };
const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });
const path = (id: string) => ({ params: { path: { id } } });

export const actions: Actions = {
	allocate: async (event) => {
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/sales-orders/{id}/allocate', path(event.params.id));
		if (!data) return failed(response.status, problem, 'Could not set stock aside.');
		const parts = [`Set aside ${data.made} allocation${data.made === 1 ? '' : 's'}.`];
		if (data.short.length) parts.push(`Still short: ${data.short.join(', ')}.`);
		return { message: parts.join(' ') } satisfies Said;
	},
	purchase: async (event) => {
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/sales-orders/{id}/purchase', path(event.params.id));
		if (!data) return failed(response.status, problem, 'Could not draft purchase orders.');
		const parts = [
			data.orders.length
				? `Drafted ${data.orders.length} purchase order${data.orders.length === 1 ? '' : 's'}.`
				: 'Nothing to buy.'
		];
		if (data.unmatched.length) parts.push(`No vendor for: ${data.unmatched.join(', ')}.`);
		return { message: parts.join(' ') } satisfies Said;
	},
	fulfil: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/sales-orders/{id}/fulfil', {
			...path(event.params.id),
			body: { version: Number(formString(form, 'version')) }
		});
		if (!data) return failed(response.status, problem, 'Could not deliver it.');
		return {
			message:
				data.status === 'fulfilled'
					? 'Delivered: the order is fulfilled.'
					: 'Delivered what was ready.'
		} satisfies Said;
	},
	agreement: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/sales-orders/{id}/agreement', {
			...path(event.params.id),
			body: {
				name: formString(form, 'name') || null,
				startsOn: formString(form, 'startsOn') || null,
				autoRenew: form.get('autoRenew') === 'on',
				activate: form.get('activate') === 'on'
			}
		});
		if (!data) return failed(response.status, problem, 'Could not make the agreement.');
		return { message: 'Agreement made.' } satisfies Said;
	},
	project: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/sales-orders/{id}/project', {
			...path(event.params.id),
			body: {
				name: formString(form, 'name') || null,
				templateId: formString(form, 'templateId') || null,
				startOn: formString(form, 'startOn') || null
			}
		});
		if (!data) return failed(response.status, problem, 'Could not start the project.');
		return { message: 'Project started.' } satisfies Said;
	},
	finish: async (event) => {
		const form = await event.request.formData();
		const cancel = formString(form, 'how') === 'cancel';
		const body = {
			version: Number(formString(form, 'version')),
			note: formString(form, 'note') || null
		};
		const client = api(event);
		const {
			data,
			error: problem,
			response
		} = cancel
			? await client.POST('/api/v1/sales-orders/{id}/cancel', { ...path(event.params.id), body })
			: await client.POST('/api/v1/sales-orders/{id}/close', { ...path(event.params.id), body });
		if (!data) return failed(response.status, problem, 'Could not do that.');
		return { message: cancel ? 'Cancelled.' : 'Closed.' } satisfies Said;
	}
};
