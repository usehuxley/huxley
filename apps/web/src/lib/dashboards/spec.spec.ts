import { describe, expect, it } from 'vitest';
import { shape, ticks } from '@usehuxley/widgets';
import { effectiveSpec, type DashboardWidget, type Source } from './spec.ts';

const source = { key: 'tickets', timeField: 'created_at' } as Source;
const widget = (over: Partial<DashboardWidget> = {}): DashboardWidget => ({
	id: 'w',
	title: 'W',
	viz: 'number',
	width: 3,
	fixedRange: false,
	spec: { source: 'tickets', measures: [{ fn: 'count' }] },
	...over
});

describe('effectiveSpec', () => {
	it('adds the dashboard range on the time field', () => {
		expect(effectiveSpec(widget(), source, 'this_month', null).filters).toEqual([
			{ field: 'created_at', op: 'within', value: 'this_month' }
		]);
	});

	it('leaves fixed widgets and their own time filters alone', () => {
		expect(effectiveSpec(widget({ fixedRange: true }), source, 'today', null).filters).toEqual([]);
		const own = widget({
			spec: {
				source: 'tickets',
				measures: [{ fn: 'count' }],
				filters: [{ field: 'created_at', op: 'within', value: 'last_year' }]
			}
		});
		expect(effectiveSpec(own, source, 'today', null).filters).toHaveLength(1);
	});

	it('narrows to an organization', () => {
		expect(effectiveSpec(widget(), source, 'today', 'org-1').orgId).toBe('org-1');
	});
});

describe('shape', () => {
	it('makes each measure a series for one grouping', () => {
		const s = shape({
			columns: [
				{ key: 'queue', label: 'Queue', kind: 'text' },
				{ key: 'count', label: 'Count', kind: 'number' }
			],
			rows: [
				['Support', 3],
				[null, 1]
			]
		});
		expect(s.x).toEqual(['Support', '(none)']);
		expect(s.split).toBe(false);
		expect(s.series).toEqual([{ key: 'count', label: 'Count', slot: 1, values: [3, 1] }]);
	});

	it('splits by a second grouping, biggest series first, folding past eight', () => {
		const rows = Array.from({ length: 10 }, (_, i) => ['Ada', `T${i}`, 10 - i]);
		const s = shape({
			columns: [
				{ key: 'technician', label: 'Technician', kind: 'text' },
				{ key: 'work_type', label: 'Work type', kind: 'text' },
				{ key: 'sum_hours', label: 'Total Hours', kind: 'number' }
			],
			rows
		});
		expect(s.split).toBe(true);
		expect(s.series.map((x) => x.label)).toEqual([
			'T0',
			'T1',
			'T2',
			'T3',
			'T4',
			'T5',
			'T6',
			'Other'
		]);
		expect(s.series.at(-1)).toMatchObject({ slot: 0, values: [3 + 2 + 1] });
	});

	it('names booleans Yes and No', () => {
		const s = shape({
			columns: [
				{ key: 'billable', label: 'Billable', kind: 'bool' },
				{ key: 'count', label: 'Count', kind: 'number' }
			],
			rows: [
				[true, 2],
				[false, 1]
			]
		});
		expect(s.x).toEqual(['Yes', 'No']);
	});
});

describe('ticks', () => {
	it('rounds the axis up to a nice step', () => {
		expect(ticks(37)).toEqual([0, 10, 20, 30, 40]);
		expect(ticks(0)).toEqual([0, 1]);
	});
});
