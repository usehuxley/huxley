import { describe, expect, it } from 'vitest';
import { styleNonce, withStyleNonce } from './csp.ts';

describe('style nonce', () => {
	it('is fresh and base64', () => {
		const a = styleNonce();
		expect(a).toMatch(/^[A-Za-z0-9+/]{22}==$/);
		expect(styleNonce()).not.toBe(a);
	});

	it('is added to style-src', () => {
		const policy = "default-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'";
		expect(withStyleNonce(policy, 'abc')).toBe(
			"default-src 'self'; style-src 'self' 'nonce-abc'; style-src-attr 'unsafe-inline'"
		);
	});

	it('prefers style-src-elem, and leaves inline-allowing policies alone', () => {
		expect(withStyleNonce("style-src 'self'; style-src-elem 'self'", 'n')).toBe(
			"style-src 'self'; style-src-elem 'self' 'nonce-n'"
		);
		const dev = "style-src 'self' 'unsafe-inline'";
		expect(withStyleNonce(dev, 'n')).toBe(dev);
		expect(withStyleNonce("default-src 'self'", 'n')).toBe("default-src 'self'");
	});
});
