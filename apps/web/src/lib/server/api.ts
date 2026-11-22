import { HUXLEY_API_URL } from '$app/env/private';
import { createHuxleyClient } from '@usehuxley/api-client';

/**
 * Server-side API client. Pass the `fetch` from the load function or request
 * event so SvelteKit can track the request and forward headers correctly.
 */
export function api(fetchFn: typeof fetch) {
	return createHuxleyClient(HUXLEY_API_URL, { fetch: fetchFn });
}
