import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { isoDay, mondayOf, weekDays } from '#lib/time/week.ts';
import type { PageServerLoad } from './$types';

const DAY = /^\d{4}-\d{2}-\d{2}$/;

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const asked = params.get('week') ?? '';
	const monday = mondayOf(DAY.test(asked) ? asked : isoDay(new Date()));
	const queueId = params.get('queue') || undefined;
	const client = api(event);
	// A day either side, so local times near midnight still land.
	const [board, queues] = await Promise.all([
		client.GET('/api/v1/dispatch/board', {
			params: { query: { from: weekDays(monday)[0], days: 8, queueId } }
		}),
		client.GET('/api/v1/queues')
	]);
	if (board.response.status === 404) error(404, 'The service desk is turned off.');
	if (!board.data) error(503, 'Could not load the board.');
	return {
		monday,
		days: weekDays(monday),
		queueId: queueId ?? '',
		people: board.data,
		queues: queues.data ?? []
	};
};
