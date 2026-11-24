import { describe, expect, it } from 'vitest';
import { byCategory, certState, certTone, levelLabel } from './labels';

describe('workforce labels', () => {
	it('names levels and certification states', () => {
		expect(levelLabel(3)).toBe('Proficient');
		expect(levelLabel(9)).toBe('Level 9');
		expect([certTone('expired'), certTone('expiring'), certTone('valid')]).toEqual([
			'danger',
			'warning',
			'success'
		]);
		expect(certState('expiring')).toBe('Expiring soon');
	});

	it('groups skills by category, uncategorized last', () => {
		const groups = byCategory([
			{ name: 'VLANs', category: 'Networking' },
			{ name: 'Typing', category: '' },
			{ name: 'Intune', category: 'Microsoft 365' }
		]);
		expect(groups.map(([c]) => c)).toEqual(['Microsoft 365', 'Networking', 'Other']);
	});
});
