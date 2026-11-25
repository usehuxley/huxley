import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

const PER_PAGE = 50;

export const load: PageServerLoad = async (event) => {
	const q = event.url.searchParams.get('q')?.trim() ?? '';
	const categoryId = event.url.searchParams.get('category') ?? '';
	const status = event.url.searchParams.get('status') ?? '';
	const page = Math.max(1, Number(event.url.searchParams.get('page')) || 1);
	const client = api(event);
	const [articles, categories] = await Promise.all([
		client.GET('/api/v1/kb/articles', {
			params: {
				query: {
					q: q || undefined,
					categoryId: categoryId || undefined,
					status: status || undefined,
					limit: PER_PAGE,
					offset: (page - 1) * PER_PAGE
				}
			}
		}),
		client.GET('/api/v1/kb/categories')
	]);
	if (articles.response.status === 404) error(404, 'The knowledge base is turned off.');
	if (!articles.data) error(503, 'Could not load the knowledge base.');
	const { user } = await event.parent();
	return {
		q,
		categoryId,
		status,
		page,
		pages: Math.max(1, Math.ceil(articles.data.total / PER_PAGE)),
		articles: articles.data.items,
		total: articles.data.total,
		categories: categories.data ?? [],
		canWrite: user.grantedPermissions.includes('kb:write'),
		canPublish: user.grantedPermissions.includes('kb:publish')
	};
};
