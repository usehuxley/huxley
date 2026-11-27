import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type Kind = components['schemas']['MappingKind'];
const KINDS: Kind[] = ['customer', 'item', 'tax_code', 'class'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const orgs = await client.GET('/api/v1/orgs');
	const all = orgs.data ?? [];
	const providers = all.filter((o) => o.kind === 'provider');
	const orgId = event.url.searchParams.get('org') || providers[0]?.id || '';
	const [settings, queue, owing] = await Promise.all([
		orgId ? client.GET('/api/v1/billing/sync-settings', { params: { query: { orgId } } }) : null,
		client.GET('/api/v1/billing/sync-queue', { params: { query: { status: 'failed' } } }),
		client.GET('/api/v1/billing/receivables')
	]);
	if (settings?.response.status === 404) error(404, 'Invoicing is turned off.');
	const target = settings?.data?.target ?? null;
	const [mappings, products, ...options] = target
		? await Promise.all([
				client.GET('/api/v1/billing/mappings', { params: { query: { orgId } } }),
				client.GET('/api/v1/catalog/products', { params: { query: { active: true } } }),
				...KINDS.map((kind) =>
					client.GET('/api/v1/billing/mappings/options', { params: { query: { orgId, kind } } })
				)
			])
		: [null, null];
	const { user } = await event.parent();
	return {
		orgId,
		providers: providers.map((o) => ({ value: o.id, label: o.name })),
		settings: settings?.data ?? null,
		target,
		mappings: mappings?.data ?? [],
		clients: all
			.filter((o) => o.kind === 'client')
			.map((o) => ({ id: o.id, name: o.name }))
			.sort((a, b) => a.name.localeCompare(b.name)),
		products: (products?.data ?? []).map((p) => ({ id: p.id, name: p.name, sku: p.sku })),
		options: Object.fromEntries(KINDS.map((k, i) => [k, options[i]?.data ?? []])),
		queue: queue.data ?? [],
		receivables: owing.data ?? [],
		canConfigure: user.grantedPermissions.includes('billing:configure'),
		canWrite: user.grantedPermissions.includes('billing:write')
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

export const actions: Actions = {
	map: async (event) => {
		const form = await event.request.formData();
		const remote = formString(form, 'remote');
		// The choice carries "id|name"; typed in, it's just the id.
		const [remoteId, ...rest] = remote.split('|');
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/billing/mappings', {
			body: {
				orgId: formString(form, 'orgId'),
				mapping: {
					kind: formString(form, 'kind') as Kind,
					localKey: formString(form, 'localKey'),
					remoteId: remoteId ?? '',
					remoteName: rest.join('|') || null
				}
			}
		});
		if (!data) return failed(response.status, problem, 'Could not save the match.');
		return { message: 'Matched.' };
	},
	unmap: async (event) => {
		const form = await event.request.formData();
		const { response, error: problem } = await api(event).DELETE('/api/v1/billing/mappings/{id}', {
			params: { path: { id: formString(form, 'id') }, query: { orgId: formString(form, 'orgId') } }
		});
		if (!response.ok) return failed(response.status, problem, 'Could not remove it.');
		return { message: 'Removed.' };
	},
	retry: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/invoices/{id}/sync', {
			params: { path: { id: formString(form, 'invoiceId') } }
		});
		if (!data) return failed(response.status, problem, 'Could not retry it.');
		return {
			message:
				data.status === 'failed' ? `Still failing: ${data.error ?? ''}` : 'Sent to sync again.'
		};
	}
};
