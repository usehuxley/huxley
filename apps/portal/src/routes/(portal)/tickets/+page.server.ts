import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { me } = await event.parent();
	const company =
		event.url.searchParams.get('scope') === 'company' &&
		me.permissions.includes('portal.ticket:read:company');
	const status = event.url.searchParams.get('status') === 'closed' ? 'closed' : 'open';
	const q = event.url.searchParams.get('q') ?? '';
	const { data, response } = await api(event).GET('/api/portal/v1/tickets', {
		params: { query: { status, scope: company ? 'company' : 'mine', q: q || undefined } }
	});
	if (response.status === 404) error(404, 'Tickets aren’t available in this portal.');
	if (!data) error(503, 'Could not load your tickets.');
	return { tickets: data, status, company, q };
};
