import { isApiProblem } from '@usehuxley/api-client';
import type { RequestEvent } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';

/** Field errors from a problem, keyed by field name. */
export function fieldErrors(problem: unknown): Record<string, string> {
	const errors: Record<string, string> = {};
	if (isApiProblem(problem)) {
		for (const e of problem.errors ?? []) errors[e.pointer.replace(/^\//, '')] = e.message;
	}
	return errors;
}

/** Who tickets can be assigned to: active people (if the caller may list them) and teams. */
export async function assignees(event: RequestEvent) {
	const client = api(event);
	const [users, teams] = await Promise.all([
		client.GET('/api/v1/users', { params: { query: { limit: 200 } } }),
		client.GET('/api/v1/teams')
	]);
	return {
		people: (users.data?.items ?? [])
			.filter((u) => u.status === 'active')
			.map((u) => ({ value: u.id, label: u.displayName })),
		teams: (teams.data ?? []).map((t) => ({ value: t.id, label: t.name }))
	};
}
