import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

const PER_PAGE = 100;

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const q = params.get('q')?.trim() ?? '';
	const typeId = params.get('type') ?? '';
	const status = params.get('status') ?? '';
	const orgId = params.get('org') ?? '';
	const page = Math.max(1, Number(params.get('page')) || 1);
	const client = api(event);
	const [assets, types, orgs] = await Promise.all([
		client.GET('/api/v1/assets', {
			params: {
				query: {
					q: q || undefined,
					typeId: typeId || undefined,
					status: status || undefined,
					orgId: orgId || undefined,
					inUse: !status,
					limit: PER_PAGE,
					offset: (page - 1) * PER_PAGE
				}
			}
		}),
		client.GET('/api/v1/asset-types'),
		client.GET('/api/v1/orgs')
	]);
	if (assets.response.status === 404) error(404, 'Assets are turned off.');
	if (!assets.data) error(503, 'Could not load the assets.');
	const { user } = await event.parent();
	return {
		q,
		typeId,
		status,
		orgId,
		page,
		pages: Math.max(1, Math.ceil(assets.data.total / PER_PAGE)),
		total: assets.data.total,
		assets: assets.data.items,
		types: types.data ?? [],
		orgs: (orgs.data ?? []).map((o) => ({ value: o.id, label: o.name })),
		canWrite: user.grantedPermissions.includes('asset:write')
	};
};
