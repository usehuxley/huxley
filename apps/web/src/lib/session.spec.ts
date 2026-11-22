import { describe, expect, it } from 'vitest';
import { parseSessionCookie, safeNext } from './session';

describe('parseSessionCookie', () => {
	it('reads the token and max-age', () => {
		expect(
			parseSessionCookie('hx_session=hxs_abc123; Path=/; Max-Age=604800; HttpOnly; SameSite=Lax')
		).toEqual({ token: 'hxs_abc123', maxAge: 604800 });
	});

	it('returns null without a session cookie', () => {
		expect(parseSessionCookie(null)).toBeNull();
		expect(parseSessionCookie('other=1; Path=/')).toBeNull();
	});

	it('ignores cookies whose names merely end in hx_session', () => {
		expect(parseSessionCookie('not_hx_session=evil; Path=/')).toBeNull();
	});
});

describe('safeNext', () => {
	it('allows same-site paths', () => {
		expect(safeNext('/tickets?status=open')).toBe('/tickets?status=open');
	});

	it.each([null, '', 'https://evil.example', '//evil.example', '/\\evil.example', 'javascript:x'])(
		'falls back to / for %s',
		(value) => {
			expect(safeNext(value)).toBe('/');
		}
	);
});
