import { error } from '@sveltejs/kit';
import { VIEWS } from '#lib/tickets/labels.ts';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

const PAGE = 50;

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const view = (VIEWS.find((v) => v.value === params.get('view')) ?? VIEWS[0]).value;
	const queueId = params.get('queue') || undefined;
	const orgId = params.get('org') || undefined;
	const search = params.get('q') ?? '';
	const page = Math.max(1, Number(params.get('page')) || 1);
	const client = api(event);
	const [tickets, queues] = await Promise.all([
		client.GET('/api/v1/tickets', {
			params: {
				query: {
					...VIEWS.find((v) => v.value === view)!.query,
					queueId,
					orgId,
					search: search || undefined,
					limit: PAGE,
					offset: (page - 1) * PAGE
				}
			}
		}),
		client.GET('/api/v1/queues')
	]);
	if (tickets.response.status === 404) error(404, 'The service desk is turned off.');
	if (!tickets.data) error(503, 'Could not load tickets.');
	return {
		tickets: tickets.data.items,
		total: tickets.data.total,
		queues: queues.data ?? [],
		view,
		queueId: queueId ?? '',
		orgId: orgId ?? '',
		search,
		page,
		pages: Math.max(1, Math.ceil(tickets.data.total / PAGE))
	};
};
