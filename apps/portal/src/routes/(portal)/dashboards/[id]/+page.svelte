<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Select } from '@usehuxley/ui';
	import { Widget, type Answer } from '@usehuxley/widgets';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let d = $derived(data.view.dashboard);

	const RANGES = [
		{ value: 'last_7_days', label: 'Last 7 days' },
		{ value: 'last_30_days', label: 'Last 30 days' },
		{ value: 'last_90_days', label: 'Last 90 days' },
		{ value: 'this_month', label: 'This month' },
		{ value: 'last_month', label: 'Last month' },
		{ value: 'this_quarter', label: 'This quarter' },
		{ value: 'this_year', label: 'This year' },
		{ value: 'last_12_months', label: 'Last 12 months' }
	];
	let options = $derived(
		RANGES.some((r) => r.value === data.view.range)
			? RANGES
			: [{ value: data.view.range, label: data.view.range.replaceAll('_', ' ') }, ...RANGES]
	);

	function pick(range: string) {
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved, with a query
		void goto(`${resolve('/(portal)/dashboards/[id]', { id: d.id })}?range=${range}`);
	}
</script>

<svelte:head>
	<title>{d.name} · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="flex flex-col gap-4">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<a href={resolve('/(portal)/dashboards')} class="text-sm text-accent hover:underline">
				Reports
			</a>
			<h1 class="mt-1 text-2xl font-semibold text-fg">{d.name}</h1>
			{#if d.description}<p class="text-fg-muted">{d.description}</p>{/if}
		</div>
		<div class="w-44">
			<Select
				aria-label="Range"
				value={data.view.range}
				{options}
				onchange={(e: Event) => pick((e.currentTarget as HTMLSelectElement).value)}
			/>
		</div>
	</div>
	<div class="grid grid-cols-1 gap-4 md:grid-cols-12">
		{#each d.widgets as w (w.id)}
			<Widget
				title={w.title}
				viz={w.viz}
				width={w.width}
				answer={(data.view.results[w.id]?.answer as Answer | null | undefined) ?? null}
				problem={data.view.results[w.id]?.problem ?? null}
			/>
		{/each}
	</div>
</div>
