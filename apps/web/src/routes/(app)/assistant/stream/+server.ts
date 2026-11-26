import { error, type RequestHandler } from '@sveltejs/kit';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';

interface StreamRequest {
	action?: string;
	id?: string;
	orgId?: string;
	text?: string;
	decisions?: Record<string, boolean>;
	context?: { kind: string; id: string } | null;
}

/** The API path and body for an action. */
function target(request: StreamRequest): [string, unknown] {
	const id = encodeURIComponent(request.id ?? '');
	switch (request.action) {
		case 'start':
			return [
				'/api/v1/assistant/conversations',
				{
					orgId: request.orgId,
					text: request.text,
					...(request.context?.id ? { context: request.context } : {})
				}
			];
		case 'say':
			return [`/api/v1/assistant/conversations/${id}/messages`, { text: request.text }];
		case 'decide':
			return [`/api/v1/assistant/conversations/${id}/decisions`, { decisions: request.decisions }];
		default:
			error(400, 'Unknown action.');
	}
}

/**
 * Streams an assistant answer (Server-Sent Events) from the API to the
 * browser: starting a conversation, sending a message, or deciding on
 * waiting actions. The page posts here so text appears as it's written.
 */
export const POST: RequestHandler = async (event) => {
	const request = (await event.request.json().catch(() => null)) as StreamRequest | null;
	if (!request) error(400, 'Send JSON.');
	const [path, body] = target(request);
	// Not tied to the browser's request: the answer finishes (and is saved)
	// even if the browser goes away.
	const upstream = await fetch(`${apiUrl}${path}`, {
		method: 'POST',
		headers: {
			...apiHeaders(event),
			accept: 'text/event-stream',
			'content-type': 'application/json'
		},
		body: JSON.stringify(body)
	}).catch(() => null);
	if (!upstream) error(502, 'The Huxley API is unavailable.');
	if (!upstream.ok || !upstream.body) {
		// A problem before answering started (validation, permissions…).
		return new Response(await upstream.text(), {
			status: upstream.status,
			headers: { 'content-type': upstream.headers.get('content-type') ?? 'application/json' }
		});
	}
	return new Response(upstream.body, {
		headers: {
			'content-type': 'text/event-stream',
			'cache-control': 'no-cache, no-transform',
			'x-accel-buffering': 'no'
		}
	});
};
