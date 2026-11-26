import { describe, expect, it } from 'vitest';
import { visibleSections } from './permissions';

describe('visibleSections', () => {
	it('shows only sections the user has permissions for', () => {
		expect(visibleSections(['org:read']).map((s) => s.key)).toEqual(['organizations']);
		// user:read shows both Users and Teams.
		expect(visibleSections(['org:read', 'user:read', 'audit:read'])).toHaveLength(4);
		expect(visibleSections(['role:read']).map((s) => s.key)).toEqual(['roles']);
		expect(visibleSections([])).toEqual([]);
	});

	it('shows asset types to those who configure the CMDB, while it is on', () => {
		expect(visibleSections(['cmdb:configure']).map((s) => s.key)).toEqual([]);
		expect(visibleSections(['cmdb:configure'], ['crm', 'cmdb']).map((s) => s.key)).toEqual([
			'asset-types',
			'asset-sync'
		]);
	});

	it('shows module sections only while the module is on', () => {
		expect(visibleSections(['servicedesk:configure']).map((s) => s.key)).toEqual([]);
		expect(
			visibleSections(['servicedesk:configure'], ['crm', 'servicedesk']).map((s) => s.key)
		).toEqual(['queues', 'sla', 'triage', 'templates', 'work-types', 'responses', 'mailboxes']);
	});
});
