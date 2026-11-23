import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

const STATUSES = ['queued', 'running', 'succeeded', 'failed', 'cancelled'] as const;
type Status = (typeof STATUSES)[number];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const raw = event.url.searchParams.get('status') ?? '';
	const status = (STATUSES as readonly string[]).includes(raw) ? (raw as Status) : undefined;
	const before = event.url.searchParams.get('before') ?? undefined;
	const [runs, workflows] = await Promise.all([
		client.GET('/api/v1/runs', { params: { query: { status, before, limit: 50 } } }),
		client.GET('/api/v1/workflows')
	]);
	if (!runs.data) error(503, 'Could not load runs.');
	return {
		runs: runs.data,
		status: status ?? '',
		workflowNames: Object.fromEntries((workflows.data ?? []).map((w) => [w.id, w.name]))
	};
};
