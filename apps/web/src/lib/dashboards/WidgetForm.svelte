<!--
	Builds one widget: what to count or add up, grouped by what, filtered
	how, and shown as what. Produces a widget with a query spec.
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import { Button, Field, Input, Select } from '@usehuxley/ui';
	import type { DashboardWidget, QuerySpec, Source } from './spec.ts';
	import { RANGES } from './spec.ts';

	type Props = {
		sources: Source[];
		widget: DashboardWidget | null;
		onsave: (w: DashboardWidget) => void;
		oncancel: () => void;
	};
	let { sources, widget, onsave, oncancel }: Props = $props();

	type Filter = NonNullable<QuerySpec['filters']>[number];
	// The form starts from the widget it was opened with.
	const start = untrack(() => ({ widget, sources }));
	const first = start.widget?.spec;
	let title = $state(start.widget?.title ?? '');
	let viz = $state(start.widget?.viz ?? 'bar');
	let width = $state(String(start.widget?.width ?? 6));
	let fixedRange = $state(start.widget?.fixedRange ?? false);
	let source = $state(first?.source ?? start.sources[0]?.key ?? '');
	let fn = $state(first?.measures?.[0]?.fn ?? 'count');
	let measureField = $state(first?.measures?.[0]?.field ?? '');
	let groupBy = $state(first?.dimensions?.[0]?.field ?? '');
	let bucket = $state(first?.dimensions?.[0]?.granularity ?? 'day');
	let splitBy = $state(first?.dimensions?.[1]?.field ?? '');
	let limit = $state(String(first?.limit ?? 20));
	let filters = $state<Filter[]>(first?.filters ? first.filters.map((f) => ({ ...f })) : []);

	let src = $derived(sources.find((s) => s.key === source));
	let fields = $derived(src?.fields ?? []);
	const kindOf = (key: string) => fields.find((f) => f.key === key)?.kind;
	let measureFields = $derived(
		fields.filter((f) =>
			fn === 'sum' || fn === 'avg'
				? f.kind === 'number'
				: fn === 'min' || fn === 'max'
					? f.kind === 'number' || f.kind === 'date' || f.kind === 'time'
					: true
		)
	);
	let timed = $derived(['date', 'time'].includes(kindOf(groupBy) ?? ''));
	let needsGroup = $derived(viz !== 'number');
	const fieldOptions = (list: typeof fields) => list.map((f) => ({ value: f.key, label: f.label }));

	const OPS = [
		{ value: 'eq', label: 'is' },
		{ value: 'neq', label: 'is not' },
		{ value: 'contains', label: 'contains' },
		{ value: 'gt', label: 'more than' },
		{ value: 'lt', label: 'less than' },
		{ value: 'within', label: 'within' },
		{ value: 'is_null', label: 'is empty' },
		{ value: 'not_null', label: 'is set' }
	];

	function filterValue(f: Filter): unknown {
		const kind = kindOf(f.field);
		const raw =
			typeof f.value === 'string' || typeof f.value === 'number' || typeof f.value === 'boolean'
				? String(f.value)
				: '';
		if (f.op === 'is_null' || f.op === 'not_null') return undefined;
		if (kind === 'bool') return raw === 'true';
		if (kind === 'number' && f.op !== 'contains') return Number(raw);
		return raw;
	}

	function save() {
		const spec: QuerySpec = {
			source,
			measures: fn === 'count' ? [{ fn: 'count' }] : [{ fn, field: measureField }],
			dimensions:
				needsGroup && groupBy
					? [
							{ field: groupBy, ...(timed ? { granularity: bucket } : {}) },
							...(splitBy && viz !== 'pie' ? [{ field: splitBy }] : [])
						]
					: [],
			filters: filters
				.filter((f) => f.field)
				.map((f) => ({ field: f.field, op: f.op, value: filterValue(f) })),
			limit: Number(limit) || 20
		};
		onsave({
			id: widget?.id ?? `w${Date.now().toString(36)}`,
			title: title.trim() || 'Untitled',
			viz,
			width: Number(width),
			fixedRange,
			spec
		});
	}
</script>

<div class="flex flex-col gap-4">
	<Field label="Title" for="w-title">
		<Input id="w-title" bind:value={title} maxlength={100} />
	</Field>
	<div class="grid gap-4 sm:grid-cols-2">
		<Field label="Data" for="w-source">
			<Select
				id="w-source"
				bind:value={source}
				options={sources.map((s) => ({ value: s.key, label: s.label }))}
			/>
		</Field>
		<Field label="Show as" for="w-viz">
			<Select
				id="w-viz"
				bind:value={viz}
				options={[
					{ value: 'number', label: 'A number' },
					{ value: 'bar', label: 'Bars' },
					{ value: 'line', label: 'A line' },
					{ value: 'area', label: 'An area' },
					{ value: 'pie', label: 'A donut' },
					{ value: 'table', label: 'A table' }
				]}
			/>
		</Field>
		<Field label="Measure" for="w-fn">
			<Select
				id="w-fn"
				bind:value={fn}
				options={[
					{ value: 'count', label: 'How many' },
					{ value: 'count_distinct', label: 'How many different' },
					{ value: 'sum', label: 'Total of' },
					{ value: 'avg', label: 'Average of' },
					{ value: 'min', label: 'Lowest or earliest' },
					{ value: 'max', label: 'Highest or latest' }
				]}
			/>
		</Field>
		{#if fn !== 'count'}
			<Field label="Of" for="w-mfield">
				<Select id="w-mfield" bind:value={measureField} options={fieldOptions(measureFields)} />
			</Field>
		{/if}
		{#if needsGroup}
			<Field label="Grouped by" for="w-group">
				<Select id="w-group" bind:value={groupBy} options={fieldOptions(fields)} />
			</Field>
			{#if timed}
				<Field label="Per" for="w-bucket">
					<Select
						id="w-bucket"
						bind:value={bucket}
						options={[
							{ value: 'day', label: 'Day' },
							{ value: 'week', label: 'Week' },
							{ value: 'month', label: 'Month' },
							{ value: 'quarter', label: 'Quarter' },
							{ value: 'year', label: 'Year' }
						]}
					/>
				</Field>
			{/if}
			{#if viz !== 'pie'}
				<Field label="Split by (optional)" for="w-split">
					<Select
						id="w-split"
						bind:value={splitBy}
						options={[{ value: '', label: 'Nothing' }, ...fieldOptions(fields)]}
					/>
				</Field>
			{/if}
		{/if}
	</div>
	<fieldset class="flex flex-col gap-2">
		<legend class="text-sm font-medium text-fg">Only where</legend>
		{#each filters as f, i (i)}
			<div class="flex flex-wrap items-center gap-2">
				<Select aria-label="Filter field" bind:value={f.field} options={fieldOptions(fields)} />
				<Select aria-label="Filter condition" bind:value={f.op} options={OPS} />
				{#if f.op === 'within'}
					<Select aria-label="Filter range" bind:value={f.value as string} options={RANGES} />
				{:else if f.op !== 'is_null' && f.op !== 'not_null'}
					<Input aria-label="Filter value" bind:value={f.value as string} class="w-40" />
				{/if}
				<Button size="sm" variant="ghost" onclick={() => filters.splice(i, 1)}>Remove</Button>
			</div>
		{/each}
		<div>
			<Button
				size="sm"
				variant="ghost"
				onclick={() => filters.push({ field: fields[0]?.key ?? '', op: 'eq', value: '' })}
			>
				Add a condition
			</Button>
		</div>
	</fieldset>
	<div class="grid gap-4 sm:grid-cols-3">
		<Field label="Width" for="w-width">
			<Select
				id="w-width"
				bind:value={width}
				options={[
					{ value: '3', label: 'Quarter' },
					{ value: '4', label: 'Third' },
					{ value: '6', label: 'Half' },
					{ value: '8', label: 'Two thirds' },
					{ value: '12', label: 'Full' }
				]}
			/>
		</Field>
		<Field label="Rows at most" for="w-limit">
			<Input id="w-limit" type="number" min="1" max="1000" bind:value={limit} />
		</Field>
		<label class="flex items-center gap-2 self-end pb-2 text-sm text-fg">
			<input type="checkbox" bind:checked={fixedRange} /> Ignore the dashboard’s range
		</label>
	</div>
	<div class="flex justify-end gap-2">
		<Button variant="ghost" onclick={oncancel}>Cancel</Button>
		<Button variant="primary" onclick={save}>Done</Button>
	</div>
</div>
