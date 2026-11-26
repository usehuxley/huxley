import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event)
		.GET('/api/portal/v1/tickets', { params: { query: { status: 'open' } } })
		.catch(() => ({ data: undefined }));
	return { open: (data ?? []).slice(0, 5) };
};
