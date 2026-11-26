import { describe, expect, it } from 'vitest';
import { parsePortalCookie, safeNext } from './session.ts';

describe('portal sessions', () => {
	it('reads the portal cookie, not the staff one', () => {
		expect(
			parsePortalCookie('hx_portal_session=hxps_abc; Path=/; Max-Age=1209600; HttpOnly')
		).toEqual({ token: 'hxps_abc', maxAge: 1209600 });
		expect(parsePortalCookie('hx_session=hxs_abc; Path=/')).toBeNull();
		expect(parsePortalCookie(null)).toBeNull();
	});
	it('only goes on to paths on this site', () => {
		expect(safeNext('/tickets')).toBe('/tickets');
		expect(safeNext('//evil.example')).toBe('/');
		expect(safeNext('https://evil.example')).toBe('/');
		expect(safeNext(null)).toBe('/');
	});
});
