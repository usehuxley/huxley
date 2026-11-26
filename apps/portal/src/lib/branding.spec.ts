import { describe, expect, it } from 'vitest';
import { accentStyle, accentText } from './branding.ts';

describe('portal branding', () => {
	it('picks readable text for the accent', () => {
		expect(accentText('#4f46e5')).toBe('#ffffff');
		expect(accentText('#fde047')).toBe('#111827');
	});
	it('only takes hex colors', () => {
		expect(accentStyle('#112233')).toContain('--hx-accent: #112233');
		expect(accentStyle('red; background: url(x)')).toBe('');
	});
});
