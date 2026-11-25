import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const id = event.params.id;
	const client = api(event);
	const [article, revisions] = await Promise.all([
		client.GET('/api/v1/kb/articles/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/kb/articles/{id}/revisions', { params: { path: { id } } })
	]);
	if (!article.data || !revisions.data) {
		error(revisions.response.status === 403 ? 403 : 404, 'Could not load the revisions.');
	}
	const number = Number(event.url.searchParams.get('n')) || revisions.data[0]?.number;
	const revision = number
		? (
				await client.GET('/api/v1/kb/articles/{id}/revisions/{number}', {
					params: { path: { id, number } }
				})
			).data
		: undefined;
	return { article: article.data, revisions: revisions.data, revision: revision ?? null };
};
