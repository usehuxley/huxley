import { describe, expect, it } from 'vitest';
import { localInstant, plusMinutes } from './appointments';

describe('appointment times', () => {
	it('read a local date and time', () => {
		const at = new Date(localInstant('2099-03-02', '09:30'));
		expect([at.getFullYear(), at.getMonth(), at.getDate(), at.getHours(), at.getMinutes()]).toEqual(
			[2099, 2, 2, 9, 30]
		);
	});

	it('add minutes', () => {
		expect(plusMinutes('2099-03-02T09:00:00.000Z', 90)).toBe('2099-03-02T10:30:00.000Z');
	});
});
