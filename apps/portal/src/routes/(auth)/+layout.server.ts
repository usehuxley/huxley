import { api } from '#lib/server/api.ts';
import { DEFAULT_BRANDING } from '#lib/branding.ts';
import type { LayoutServerLoad } from './$types';

/** The branding for this portal's address, before anyone signs in. */
export const load: LayoutServerLoad = async (event) => {
	const { data } = await api(event)
		.GET('/api/portal/v1/branding', { params: { query: { host: event.url.host } } })
		.catch(() => ({ data: undefined }));
	return { branding: data ?? DEFAULT_BRANDING };
};
