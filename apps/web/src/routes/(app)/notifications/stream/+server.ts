import type { RequestHandler } from '@sveltejs/kit';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';

/** Proxies the API's notification stream (Server-Sent Events) to the browser. */
export const GET: RequestHandler = async (event) => {
	const upstream = await fetch(`${apiUrl}/api/v1/notifications/stream`, {
		headers: { ...apiHeaders(event), accept: 'text/event-stream' },
		signal: event.request.signal
	}).catch(() => null);
	if (!upstream?.ok || !upstream.body) {
		return new Response(null, { status: upstream?.status ?? 502 });
	}
	return new Response(upstream.body, {
		headers: {
			'content-type': 'text/event-stream',
			'cache-control': 'no-cache, no-transform',
			// Tell proxies such as nginx not to buffer the stream.
			'x-accel-buffering': 'no'
		}
	});
};
