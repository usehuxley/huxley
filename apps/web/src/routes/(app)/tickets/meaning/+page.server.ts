import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { PageServerLoad } from './$types';

/** Tickets found by meaning: what the API finds for `q`, read by the current organization's embeddings provider. */
export const load: PageServerLoad = async (event) => {
	const { currentOrg } = await event.parent();
	const q = event.url.searchParams.get('q')?.trim() ?? '';
	if (q.length < 2) return { q, result: null, message: null };
	const client = api(event);
	const orgId =
		currentOrg?.id ??
		(await client.GET('/api/v1/orgs')).data?.[0]?.id ??
		error(404, 'No organization.');
	const {
		data,
		error: problem,
		response
	} = await client.GET('/api/v1/search/tickets', {
		params: { query: { q, orgId } }
	});
	if (response.status === 403 || response.status === 404) {
		return {
			q,
			result: null,
			message: 'Searching by meaning needs permission to use AI here (ai:use).'
		};
	}
	if (!data) return { q, result: null, message: problemMessage(problem, 'The search failed.') };
	return { q, result: data, message: data.message ?? null };
};
