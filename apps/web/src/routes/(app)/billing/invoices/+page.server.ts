import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const status = (['draft', 'approved', 'sent', 'void'] as const).find(
		(s) => s === params.get('status')
	);
	const { data, response } = await api(event).GET('/api/v1/invoices', {
		params: {
			query: { status, runId: params.get('runId') || undefined, q: params.get('q') || undefined }
		}
	});
	if (response.status === 404) error(404, 'Invoicing is turned off.');
	if (!data) error(response.status === 403 ? 403 : 503, 'Could not load invoices.');
	return {
		invoices: data,
		status: status ?? '',
		q: params.get('q') ?? '',
		runId: params.get('runId') ?? ''
	};
};
