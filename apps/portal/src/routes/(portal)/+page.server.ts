import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const [tickets, approvals] = await Promise.all([
		api(event)
			.GET('/api/portal/v1/tickets', { params: { query: { status: 'open' } } })
			.catch(() => ({ data: undefined })),
		api(event)
			.GET('/api/portal/v1/approvals')
			.catch(() => ({ data: undefined }))
	]);
	return {
		open: (tickets.data ?? []).slice(0, 5),
		waiting: (approvals.data ?? []).filter((a) => a.status === 'pending').length
	};
};
