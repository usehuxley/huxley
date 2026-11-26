/** An analytics answer (`POST /api/v1/analytics/query`). */
export interface Answer {
	columns: { key: string; label: string; kind: 'text' | 'number' | 'bool' | 'date' | 'time' }[];
	rows: unknown[][];
	truncated?: boolean;
	cached?: boolean;
}

export type Viz = 'number' | 'table' | 'bar' | 'line' | 'area' | 'pie';

export interface Series {
	key: string;
	label: string;
	/** Its color slot (1-8), or 0 for "Other". */
	slot: number;
	values: (number | null)[];
}

/** A chart's data: categories (or times) along one axis, and series. */
export interface Shaped {
	x: string[];
	xKind: Answer['columns'][number]['kind'];
	xLabel: string;
	measureLabel: string;
	/** Split by a second grouping: the series add up (stack). */
	split: boolean;
	series: Series[];
}

/** At most this many series get their own color; the rest are "Other". */
export const MAX_SERIES = 8;

const numberFormat = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });

/** A cell as text: numbers grouped, booleans as Yes/No, dates short. */
export function formatValue(value: unknown, kind: string): string {
	if (value === null || value === undefined || value === '') return '(none)';
	if (kind === 'number' && typeof value === 'number') return numberFormat.format(value);
	if (kind === 'bool') return value ? 'Yes' : 'No';
	if (kind === 'date' && typeof value === 'string') {
		const d = new Date(`${value.slice(0, 10)}T00:00:00`);
		return Number.isNaN(d.getTime())
			? value
			: d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
	}
	if (kind === 'time' && typeof value === 'string') {
		const d = new Date(value);
		return Number.isNaN(d.getTime())
			? value
			: d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}
	if (typeof value === 'string') return value;
	if (typeof value === 'number' || typeof value === 'boolean') return String(value);
	return JSON.stringify(value);
}

const num = (v: unknown): number | null =>
	typeof v === 'number'
		? v
		: typeof v === 'string' && v !== '' && !Number.isNaN(Number(v))
			? Number(v)
			: null;

/**
 * Shapes an answer for a chart. One grouping: each measure is a series. Two
 * groupings: the second splits the first measure into series (the biggest
 * eight keep their colors; the rest fold into "Other").
 */
export function shape(answer: Answer): Shaped {
	const columns = answer.columns;
	const dims = columns.filter((c, i) => i < columns.length && !isMeasure(columns, i));
	const measures = columns.filter((_, i) => isMeasure(columns, i));
	const xCol = dims[0] ?? columns[0];
	const xIndex = columns.indexOf(xCol);
	const x: string[] = [];
	const xKey = (row: unknown[]) => formatValue(row[xIndex], xCol.kind);
	for (const row of answer.rows) {
		const k = xKey(row);
		if (!x.includes(k)) x.push(k);
	}
	if (dims.length < 2) {
		return {
			x,
			xKind: xCol.kind,
			xLabel: xCol.label,
			measureLabel: measures.map((m) => m.label).join(', '),
			split: false,
			series: measures.slice(0, MAX_SERIES).map((m, i) => {
				const mi = columns.indexOf(m);
				const values = x.map((k) => {
					const row = answer.rows.find((r) => xKey(r) === k);
					return row ? num(row[mi]) : null;
				});
				return { key: m.key, label: m.label, slot: i + 1, values };
			})
		};
	}
	const splitCol = dims[1];
	const si = columns.indexOf(splitCol);
	const mi = columns.indexOf(measures[0]);
	const totals = new Map<string, number>();
	for (const row of answer.rows) {
		const s = formatValue(row[si], splitCol.kind);
		totals.set(s, (totals.get(s) ?? 0) + (num(row[mi]) ?? 0));
	}
	// Biggest first, so colors follow the series' size in this answer.
	const names = [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([s]) => s);
	const kept = names.length > MAX_SERIES ? names.slice(0, MAX_SERIES - 1) : names;
	const series: Series[] = kept.map((name, i) => ({
		key: name,
		label: name,
		slot: i + 1,
		values: x.map(() => null)
	}));
	const other: Series = { key: '__other', label: 'Other', slot: 0, values: x.map(() => null) };
	for (const row of answer.rows) {
		const xi = x.indexOf(xKey(row));
		const name = formatValue(row[si], splitCol.kind);
		const target = series.find((s) => s.key === name) ?? other;
		target.values[xi] = (target.values[xi] ?? 0) + (num(row[mi]) ?? 0);
	}
	if (other.values.some((v) => v !== null)) series.push(other);
	return {
		x,
		xKind: xCol.kind,
		xLabel: xCol.label,
		measureLabel: measures[0]?.label ?? '',
		split: true,
		series
	};
}

/** Measures come after the groupings, and their keys name an aggregate. */
function isMeasure(columns: Answer['columns'], i: number): boolean {
	const key = columns[i].key;
	return /^(count|count_distinct_|sum_|avg_|min_|max_)/.test(key);
}

/** The largest stacked (or single) value, for an axis. */
export function maxOf(shaped: Shaped, stacked: boolean): number {
	let max = 0;
	for (let i = 0; i < shaped.x.length; i++) {
		const values = shaped.series.map((s) => s.values[i] ?? 0);
		const v = stacked ? values.reduce((a, b) => a + b, 0) : Math.max(0, ...values);
		max = Math.max(max, v);
	}
	return max;
}

/** A "nice" axis top and ticks for `max`. */
export function ticks(max: number, count = 4): number[] {
	if (max <= 0) return [0, 1];
	const raw = max / count;
	const magnitude = 10 ** Math.floor(Math.log10(raw));
	const step = [1, 2, 2.5, 5, 10].map((m) => m * magnitude).find((s) => s >= raw) ?? raw;
	const top = Math.ceil(max / step) * step;
	const out: number[] = [];
	for (let v = 0; v <= top + step / 2; v += step) out.push(Number(v.toFixed(10)));
	return out;
}

/** A series' color. */
export function color(slot: number): string {
	return slot === 0 ? 'var(--viz-other)' : `var(--viz-${slot})`;
}
