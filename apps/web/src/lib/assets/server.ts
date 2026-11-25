import type { RequestEvent } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** An organization's sites and contacts, for an asset's form. */
export async function orgOptions(event: RequestEvent, orgId: string) {
	const client = api(event);
	const [sites, contacts] = await Promise.all([
		client.GET('/api/v1/orgs/{id}/sites', { params: { path: { id: orgId } } }),
		client.GET('/api/v1/orgs/{id}/contacts', {
			params: { path: { id: orgId }, query: { limit: 200 } }
		})
	]);
	return {
		sites: (sites.data ?? []).map((s) => ({ value: s.id, label: s.name })),
		contacts: (contacts.data?.items ?? []).map((c) => ({
			value: c.id,
			label: `${c.firstName} ${c.lastName}`.trim() || c.email || 'Contact'
		}))
	};
}
