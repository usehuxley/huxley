import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const status = (['draft', 'sent', 'accepted', 'declined'] as const).find(
		(s) => s === params.get('status')
	);
	const { data, response } = await api(event).GET('/api/v1/quotes', {
		params: { query: { status, q: params.get('q') || undefined } }
	});
	if (response.status === 404) error(404, 'Sales is turned off.');
	if (!data) error(503, 'Could not load quotes.');
	const { user } = await event.parent();
	return {
		quotes: data,
		status: status ?? '',
		q: params.get('q') ?? '',
		canWrite: user.grantedPermissions.includes('sales:write')
	};
};
