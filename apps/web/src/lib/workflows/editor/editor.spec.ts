import { describe, expect, it } from 'vitest';
import { getField, newStep, pathKey, setField, stepIds, summary } from './catalog.ts';
import { ownerOf } from './problems.ts';

describe('visual editor helpers', () => {
	it('finds the step a diagnostic belongs to', () => {
		expect(ownerOf('spec.steps[0].id')).toBe('spec.steps[0]');
		expect(ownerOf('spec.steps[2].cases[0].steps[1].output')).toBe(
			'spec.steps[2].cases[0].steps[1]'
		);
		expect(ownerOf('spec.steps[1].branches[0].steps[3].with.url')).toBe(
			'spec.steps[1].branches[0].steps[3]'
		);
		expect(ownerOf('spec.steps[3].then[0].retry.on[1]')).toBe('spec.steps[3].then[0]');
		expect(ownerOf('spec.on_error[0].with.title')).toBe('spec.on_error[0]');
		expect(ownerOf('metadata.key')).toBeUndefined();
		expect(ownerOf('spec.triggers[0].cron')).toBeUndefined();
	});

	it('paths match the server format', () => {
		expect(pathKey(['spec', 'steps', 2, 'cases', 0, 'steps', 1])).toBe(
			'spec.steps[2].cases[0].steps[1]'
		);
	});

	it('new steps get unique ids and templates', () => {
		const doc = { spec: { steps: [{ id: 'http_request_1', type: 'http.request' }] } };
		const step = newStep('http.request', doc);
		expect(step.id).toBe('http_request_2');
		expect(getField(step, 'with.method')).toBe('GET');
		expect(
			stepIds({
				spec: { steps: [step, { id: 'x', type: 'if', then: [{ id: 'y', type: 'fail' }] }] }
			})
		).toEqual(new Set(['http_request_2', 'x', 'y']));
		// Templates are copied, not shared.
		const other = newStep('http.request', doc);
		setField(other, 'with.url', 'https://x');
		expect(getField(step, 'with.url')).toBe('https://');
	});

	it('sets and clears nested fields', () => {
		const step: Record<string, unknown> = { id: 'a', type: 'http.request' };
		setField(step, 'with.url', 'https://api');
		expect(step).toEqual({ id: 'a', type: 'http.request', with: { url: 'https://api' } });
		setField(step, 'with.url', '');
		expect(step).toEqual({ id: 'a', type: 'http.request', with: {} });
		expect(summary({ id: 'w', type: 'wait', duration: '5m' })).toBe('5m');
	});
});
