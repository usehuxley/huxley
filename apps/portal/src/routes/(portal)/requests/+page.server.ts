import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const [catalog, requests] = await Promise.all([
		api(event).GET('/api/portal/v1/catalog'),
		api(event).GET('/api/portal/v1/requests')
	]);
	return {
		forms: catalog.data ?? [],
		requests: requests.data ?? [],
		sent: event.url.searchParams.has('sent')
	};
};
