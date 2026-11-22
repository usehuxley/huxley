import { describe, expect, it } from 'vitest';
import { statusBadge, toPlatformStatus } from './status';

describe('toPlatformStatus', () => {
	it('reports ready with the API version', () => {
		expect(toPlatformStatus({ ok: true, version: '0.1.0' })).toEqual({
			state: 'ready',
			version: '0.1.0'
		});
	});

	it('reports degraded with the problem detail', () => {
		expect(toPlatformStatus({ ok: false, detail: 'Database is unreachable.' })).toEqual({
			state: 'degraded',
			detail: 'Database is unreachable.'
		});
	});

	it('reports unreachable when the API did not answer', () => {
		expect(toPlatformStatus(null)).toEqual({ state: 'unreachable' });
	});
});

describe('statusBadge', () => {
	it('uses a danger tone when the API is unreachable', () => {
		expect(statusBadge({ state: 'unreachable' }).tone).toBe('danger');
	});
});
