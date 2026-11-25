import type { RequestEvent } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

export type ActionForm = { id: string; name: string; description: string | null };

/**
 * Forms offered as Actions on `kind` that can run for `orgId`: those of the
 * organization or one above it, not archived.
 */
export async function actionsFor(
	event: RequestEvent,
	kind: 'client' | 'contact' | 'ticket',
	orgId: string
): Promise<ActionForm[]> {
	const client = api(event);
	const [forms, orgs] = await Promise.all([
		client.GET('/api/v1/forms'),
		client.GET('/api/v1/orgs')
	]);
	const parents = new Map((orgs.data ?? []).map((o) => [o.id, o.parentId ?? null]));
	const above = new Set<string>();
	for (let at: string | null = orgId; at && !above.has(at); at = parents.get(at) ?? null)
		above.add(at);
	return (forms.data ?? [])
		.filter((f) => f.actionFor === kind && !f.archived && above.has(f.orgId))
		.map((f) => ({ id: f.id, name: f.name, description: f.description ?? null }));
}
