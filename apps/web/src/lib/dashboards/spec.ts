import type { components } from '@usehuxley/api-client';

export type QuerySpec = components['schemas']['QuerySpec'];
export type DashboardWidget = components['schemas']['Widget'];
export type Source = components['schemas']['AnalyticsSource'];

/** The ranges a dashboard can show, with labels. */
export const RANGES: { value: string; label: string }[] = [
	{ value: 'today', label: 'Today' },
	{ value: 'yesterday', label: 'Yesterday' },
	{ value: 'last_7_days', label: 'Last 7 days' },
	{ value: 'last_30_days', label: 'Last 30 days' },
	{ value: 'last_90_days', label: 'Last 90 days' },
	{ value: 'this_week', label: 'This week' },
	{ value: 'last_week', label: 'Last week' },
	{ value: 'this_month', label: 'This month' },
	{ value: 'last_month', label: 'Last month' },
	{ value: 'this_quarter', label: 'This quarter' },
	{ value: 'last_quarter', label: 'Last quarter' },
	{ value: 'this_year', label: 'This year' },
	{ value: 'last_year', label: 'Last year' },
	{ value: 'last_12_months', label: 'Last 12 months' }
];

/**
 * The spec a widget runs on a dashboard: limited to the dashboard's range
 * on the source's time field (unless the widget fixes its own, or already
 * filters that field), and to one organization when one is chosen.
 */
export function effectiveSpec(
	widget: DashboardWidget,
	source: Source | undefined,
	range: string,
	orgId: string | null
): QuerySpec {
	const spec: QuerySpec = { ...widget.spec, filters: [...(widget.spec.filters ?? [])] };
	const field = source?.timeField;
	if (field && !widget.fixedRange && !spec.filters!.some((f) => f.field === field)) {
		spec.filters!.push({ field, op: 'within', value: range });
	}
	if (orgId) spec.orgId = orgId;
	return spec;
}
