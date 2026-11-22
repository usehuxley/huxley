import { describe, expect, it } from 'vitest';
import { visibleSections } from './permissions';

describe('visibleSections', () => {
	it('shows only sections the user has permissions for', () => {
		expect(visibleSections(['org:read']).map((s) => s.key)).toEqual(['organizations']);
		expect(visibleSections(['org:read', 'user:read', 'audit:read'])).toHaveLength(3);
		expect(visibleSections([])).toEqual([]);
	});
});
