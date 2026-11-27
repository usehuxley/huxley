import { api } from '#lib/server/api.ts';
import { toPlatformStatus } from '#lib/status.ts';
import { isApiProblem } from '@usehuxley/api-client';
import type { PageServerLoad } from './$types';

async function platformStatus(event: Parameters<PageServerLoad>[0]) {
	try {
		const { data, error } = await api(event).GET('/readyz', {
			signal: AbortSignal.timeout(3000)
		});
		if (data) return toPlatformStatus({ ok: true, version: data.version });
		return toPlatformStatus({
			ok: false,
			detail: isApiProblem(error) ? (error.detail ?? undefined) : undefined
		});
	} catch {
		return toPlatformStatus(null);
	}
}

/** Your day: what needs you, what waits on you, and what you started. */
export const load: PageServerLoad = async (event) => {
	const { user } = await event.parent();
	const can = (p: string) => user.grantedPermissions.includes(p);
	const desk = user.activeModules.includes('servicedesk') && can('ticket:read');
	const client = api(event);
	const [status, work, approvals, runs] = await Promise.all([
		platformStatus(event),
		desk
			? client
					.GET('/api/v1/my-work')
					.then((r) => r.data ?? null)
					.catch(() => null)
			: null,
		client
			.GET('/api/v1/approvals')
			.then((r) => (r.data ?? []).filter((a) => a.status === 'pending'))
			.catch(() => []),
		can('workflow:read')
			? client
					.GET('/api/v1/runs', { params: { query: { limit: 30 } } })
					.then((r) => (r.data ?? []).filter((run) => run.actorId === user.id).slice(0, 5))
					.catch(() => [])
			: []
	]);
	// The workflows' names, for the runs.
	const names = new Map<string, string>();
	if (runs.length > 0) {
		const workflows = await client
			.GET('/api/v1/workflows')
			.then((r) => r.data ?? [])
			.catch(() => []);
		for (const w of workflows) names.set(w.id, w.name);
	}
	return {
		status,
		work,
		approvals,
		runs: runs.map((run) => ({
			...run,
			workflowName: (run.workflowId && names.get(run.workflowId)) || 'Test run'
		}))
	};
};
