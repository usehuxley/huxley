/**
 * A nonce for `<style>` elements the page adds itself. CodeMirror (the code
 * editor) injects its styles as `<style>` tags, which `style-src 'self'`
 * blocks; SvelteKit only puts a style nonce in the policy when it renders
 * inline styles of its own. So each response gets one: it's added to the
 * policy here and handed to the page in a meta tag, and only tags carrying it
 * are allowed (no `'unsafe-inline'` for style elements).
 */

/** The placeholder in app.html the nonce replaces. */
export const STYLE_NONCE_PLACEHOLDER = '%huxley.style_nonce%';

export function styleNonce(): string {
	const bytes = new Uint8Array(16);
	crypto.getRandomValues(bytes);
	return btoa(String.fromCharCode(...bytes));
}

/** The policy with the nonce added to the directive that governs style
 * elements (`style-src-elem`, else `style-src`); unchanged if neither is set
 * or it already allows inline styles. */
export function withStyleNonce(policy: string, nonce: string): string {
	const parts = policy
		.split(';')
		.map((p) => p.trim())
		.filter(Boolean);
	let at = parts.findIndex((p) => p.startsWith('style-src-elem '));
	if (at < 0) at = parts.findIndex((p) => p.startsWith('style-src '));
	if (at < 0 || parts[at].includes("'unsafe-inline'")) return policy;
	parts[at] = `${parts[at]} 'nonce-${nonce}'`;
	return parts.join('; ');
}
