import { describe, expect, it } from 'vitest';
import { visibleSections } from './permissions';

describe('visibleSections', () => {
	it('shows only sections the user has permissions for', () => {
		expect(visibleSections(['org:read']).map((s) => s.key)).toEqual(['organizations']);
		// user:read shows both Users and Teams.
		expect(visibleSections(['org:read', 'user:read', 'audit:read'])).toHaveLength(4);
		expect(visibleSections([])).toEqual([]);
	});
});
