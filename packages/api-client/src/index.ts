/**
 * Typed Huxley API client.
 *
 * `schema.d.ts` is generated from the server's OpenAPI document with
 * `pnpm api-client:generate`. Never edit it by hand; CI fails if it is stale.
 */
import createClient, { type ClientOptions } from 'openapi-fetch';
import type { components, paths } from './schema';

export type { components, paths };

/** RFC 9457 problem details returned by every API error. */
export type ApiProblem = components['schemas']['ApiError'];

export type HuxleyClient = ReturnType<typeof createClient<paths>>;

/**
 * Creates a client. `baseUrl` is the API origin, e.g. `http://localhost:8080`.
 * Server-side code passes SvelteKit's `fetch` so requests are traced and
 * cookies are forwarded correctly.
 */
export function createHuxleyClient(baseUrl: string, options: Omit<ClientOptions, 'baseUrl'> = {}) {
	return createClient<paths>({ ...options, baseUrl });
}

/** Type guard for problem-details error bodies. */
export function isApiProblem(value: unknown): value is ApiProblem {
	return (
		typeof value === 'object' &&
		value !== null &&
		'type' in value &&
		'title' in value &&
		'status' in value
	);
}
