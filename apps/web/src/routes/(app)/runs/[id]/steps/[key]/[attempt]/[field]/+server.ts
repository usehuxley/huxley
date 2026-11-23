import type { RequestHandler } from '@sveltejs/kit';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';

/**
 * Downloads a step attempt's full input or output as JSON. Large values are
 * kept in object storage, and the run page shows only their size.
 */
export const GET: RequestHandler = async (event) => {
	const { id = '', key = '', attempt = '', field = '' } = event.params;
	if (field !== 'input' && field !== 'output') return new Response('Not found', { status: 404 });
	const path = [id, 'steps', key, attempt, field].map(encodeURIComponent).join('/');
	const upstream = await fetch(`${apiUrl}/api/v1/runs/${path}`, {
		headers: apiHeaders(event),
		signal: event.request.signal
	}).catch(() => null);
	if (!upstream?.ok || !upstream.body) {
		return new Response('Not found', { status: upstream?.status === 403 ? 403 : 404 });
	}
	const name = `${key.replace(/[^\w.-]+/g, '_')}-${attempt}-${field}.json`;
	return new Response(upstream.body, {
		headers: {
			'content-type': 'application/json',
			'content-disposition': `attachment; filename="${name}"`,
			'x-content-type-options': 'nosniff',
			'cache-control': 'private, no-store'
		}
	});
};
