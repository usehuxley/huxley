import type { RequestEvent } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** What the mailbox form offers: ticket types, queues, organizations. */
export async function formOptions(event: RequestEvent) {
	const client = api(event);
	const [types, queues, orgs] = await Promise.all([
		client.GET('/api/v1/servicedesk/types'),
		client.GET('/api/v1/queues'),
		client.GET('/api/v1/orgs')
	]);
	const active = (orgs.data ?? []).filter((o) => o.status === 'active');
	return {
		types: (types.data ?? []).map((t) => ({ value: t.key, label: t.name })),
		queues: (queues.data ?? []).map((q) => ({ value: q.id, label: q.name })),
		orgs: active.map((o) => ({ value: o.id, label: o.name })),
		providers: active
			.filter((o) => o.kind === 'provider')
			.map((o) => ({ value: o.id, label: o.name }))
	};
}
