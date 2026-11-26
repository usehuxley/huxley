import { describe as group, expect, it } from 'vitest';
import { describe } from './activity';

group('ticket activity', () => {
	it('describes each kind', () => {
		expect(describe('created', { title: 'x' })).toBe('created the ticket');
		const names: Record<string, string> = { new: 'New', in_progress: 'In progress' };
		expect(describe('status', { from: 'new', to: 'in_progress' }, (k) => names[k])).toBe(
			'changed the status from New to In progress'
		);
		expect(describe('note', { kind: 'internal' })).toBe('added an internal note');
		expect(describe('link', { linked: 'T-2', as: 'parent' })).toBe('made T-2 its parent');
		expect(describe('link', { linked: 'T-3', as: 'problem' })).toBe('linked T-3 as its problem');
		expect(describe('link', { linked: 'T-4', as: 'incident' })).toBe('linked T-4 as an incident it causes');
		expect(describe('link', { linked: 'T-3', as: 'related' })).toBe('linked T-3');
		expect(describe('link', { unlinked: 'T-3' })).toBe('unlinked T-3');
		expect(describe('merge', { merged: 'T-4' })).toBe('merged T-4 into this ticket');
		expect(describe('action', { form: 'Reset MFA', runs: [] })).toBe('ran Reset MFA');
		expect(describe('status', { from: 'new', to: 'closed', mergedInto: 'T-1' })).toBe(
			'merged this ticket into T-1'
		);
		expect(describe('note', { kind: 'public' })).toBe('replied');
		expect(describe('sla', { clock: 'response', alert: 'breached' })).toBe(
			'noted that the first response missed its SLA target'
		);
		expect(describe('sla', { clock: 'resolution', alert: 'warning' })).toBe(
			'warned that resolution is nearly due'
		);
	});

	it('describes field changes', () => {
		expect(
			describe('updated', {
				impact: { from: 2, to: 1 },
				priority: { from: 'p3', to: 'p2' },
				description: { changed: true }
			})
		).toBe(
			'changed the impact from medium to high, changed the priority from P3 to P2, edited the description'
		);
		expect(describe('assigned', { assignee: { from: null, to: 'Ada' } })).toBe('assigned Ada');
		expect(describe('assigned', { assignee: { from: 'Ada', to: null } })).toBe('unassigned Ada');
		expect(describe('updated', { queue: { from: null, to: 'Helpdesk' } })).toBe(
			'changed the queue from none to Helpdesk'
		);
		expect(describe('updated', {})).toBe('updated the ticket');
	});
});
