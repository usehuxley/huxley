import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';

/** Container liveness probe. Does not call the API, so an API outage doesn't restart the web tier. */
export const GET: RequestHandler = () => json({ status: 'ok' });
