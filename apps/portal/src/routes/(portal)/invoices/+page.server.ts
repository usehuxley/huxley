import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/portal/v1/invoices');
	return { invoices: data ?? [] };
};
