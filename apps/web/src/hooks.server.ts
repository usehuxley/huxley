import type { Handle } from '@sveltejs/kit/hooks';

/**
 * Security headers for every response. The Content Security Policy is set by
 * SvelteKit itself (see `csp` in vite.config.ts) so it can add nonces.
 * HSTS is left to the TLS-terminating proxy (Caddy in the compose stack).
 */
const SECURITY_HEADERS: Record<string, string> = {
	'X-Content-Type-Options': 'nosniff',
	'X-Frame-Options': 'DENY',
	'Referrer-Policy': 'strict-origin-when-cross-origin',
	'Permissions-Policy': 'camera=(), microphone=(), geolocation=(), payment=()',
	'Cross-Origin-Opener-Policy': 'same-origin'
};

export const handle: Handle = async ({ event, resolve }) => {
	const response = await resolve(event);
	for (const [name, value] of Object.entries(SECURITY_HEADERS)) {
		if (!response.headers.has(name)) response.headers.set(name, value);
	}
	return response;
};
