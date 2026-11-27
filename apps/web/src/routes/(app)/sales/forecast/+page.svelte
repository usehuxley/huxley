<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ChevronLeft, ChevronRight, Target } from '@lucide/svelte';
	import { Button, Card, Dialog, EmptyState, Field, Input, Select, toast } from '@usehuxley/ui';
	import { money } from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let open = $state(false);
	let who = $state('');
	let amount = $state('');
	// Quotas are kept per month or quarter; a year view shows their total.
	let quotaMonths = $derived(data.span === 'month' ? 1 : 3);

	const totals = $derived(
		data.rows.reduce(
			(t, r) => ({
				quota: t.quota + Number(r.quota ?? 0),
				won: t.won + Number(r.won),
				commit: t.commit + Number(r.commit),
				bestCase: t.bestCase + Number(r.bestCase),
				pipeline: t.pipeline + Number(r.pipeline),
				weighted: t.weighted + Number(r.weighted)
			}),
			{ quota: 0, won: 0, commit: 0, bestCase: 0, pipeline: 0, weighted: 0 }
		)
	);
	const currency = $derived(data.rows[0]?.currency ?? 'USD');
	const href = (at: string) => `${resolve('/(app)/sales/forecast')}?span=${data.span}&at=${at}`;
	const pct = (won: string | number, quota: string | null | undefined) =>
		quota && Number(quota) > 0 ? Math.min(100, (Number(won) / Number(quota)) * 100) : 0;
</script>

<svelte:head>
	<title>Forecast · Sales · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div class="flex flex-wrap items-end gap-4">
		<div>
			<h1 class="text-[26px] leading-tight font-extrabold tracking-tight text-fg">Forecast</h1>
			<p class="mt-1 text-sm text-fg-subtle">
				What's won and what's expected to close, against quota (first-year values).
			</p>
		</div>
		<div class="ml-auto flex flex-wrap items-center gap-2">
			<nav class="flex rounded-control border border-border" aria-label="Period length">
				{#each ['month', 'quarter', 'year'] as s (s)}
					<a
						href="{resolve('/(app)/sales/forecast')}?span={s}&at={data.period.from}"
						class="px-3 py-1.5 text-sm {data.span === s
							? 'bg-accent text-accent-fg'
							: 'text-fg-muted hover:text-fg'}"
						aria-current={data.span === s ? 'page' : undefined}
					>
						{s[0].toUpperCase() + s.slice(1)}
					</a>
				{/each}
			</nav>
			<Button href={href(data.prev)} variant="ghost" aria-label="Previous period">
				<ChevronLeft size={16} />
			</Button>
			<span class="min-w-28 text-center text-sm font-semibold text-fg">{data.period.label}</span>
			<Button href={href(data.next)} variant="ghost" aria-label="Next period">
				<ChevronRight size={16} />
			</Button>
			{#if data.canConfigure && data.span !== 'year'}
				<Button onclick={() => ((who = data.people[0]?.value ?? ''), (amount = ''), (open = true))}>
					<Target size={16} /> Set a quota
				</Button>
			{/if}
		</div>
	</div>

	<div class="grid gap-4 sm:grid-cols-3 lg:grid-cols-6">
		{#each [['Quota', totals.quota], ['Won', totals.won], ['Commit', totals.commit], ['Best case', totals.bestCase], ['Pipeline', totals.pipeline], ['Weighted', totals.weighted]] as [label, value] (label)}
			<Card>
				<p class="text-xs font-bold text-fg-subtle">{label}</p>
				<p class="mt-1 font-mono text-lg font-semibold tabular-nums">{money(value, currency)}</p>
			</Card>
		{/each}
	</div>

	{#if data.rows.length === 0}
		<EmptyState
			title="Nothing in this period"
			description="Deals expected to close or won in the period, and quotas set for it, show here."
		/>
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full min-w-[860px] text-left text-sm" aria-label="Forecast by owner">
				<thead class="border-b border-border text-xs font-bold text-fg-subtle">
					<tr>
						<th scope="col" class="px-4 py-2.5">Owner</th>
						<th scope="col" class="px-4 py-2.5 text-right">Quota</th>
						<th scope="col" class="w-48 px-4 py-2.5">Won</th>
						<th scope="col" class="px-4 py-2.5 text-right">Commit</th>
						<th scope="col" class="px-4 py-2.5 text-right">Best case</th>
						<th scope="col" class="px-4 py-2.5 text-right">Pipeline</th>
						<th scope="col" class="px-4 py-2.5 text-right">Weighted</th>
						<th scope="col" class="px-4 py-2.5 text-right">Open deals</th>
					</tr>
				</thead>
				<tbody>
					{#each data.rows as r (`${r.ownerId}-${r.currency}`)}
						<tr class="border-b border-border last:border-0">
							<td class="px-4 py-2.5 font-semibold text-fg">{r.ownerName ?? 'No owner'}</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{#if data.canConfigure && r.ownerId && data.span !== 'year'}
									<button
										type="button"
										class="hover:text-accent hover:underline"
										onclick={() => ((who = r.ownerId!), (amount = r.quota ?? ''), (open = true))}
									>
										{r.quota ? money(r.quota, r.currency) : 'Set'}
									</button>
								{:else}
									{r.quota ? money(r.quota, r.currency) : '—'}
								{/if}
							</td>
							<td class="px-4 py-2.5">
								<span class="font-mono tabular-nums">{money(r.won, r.currency)}</span>
								{#if r.attainment}<span class="ml-1 text-xs text-fg-subtle">
										{r.attainment}%
									</span>{/if}
								{#if r.quota}
									<span class="mt-1 block h-1.5 rounded bg-surface-sunken" aria-hidden="true">
										<span
											class="block h-1.5 rounded bg-success"
											style="width: {pct(r.won, r.quota)}%"
										></span>
									</span>
								{/if}
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(r.commit, r.currency)}
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(r.bestCase, r.currency)}
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(r.pipeline, r.currency)}
							</td>
							<td class="px-4 py-2.5 text-right font-mono text-fg-muted tabular-nums">
								{money(r.weighted, r.currency)}
							</td>
							<td class="px-4 py-2.5 text-right tabular-nums">{r.openCount}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>

<Dialog bind:open title="Quota for {data.period.label}">
	<form
		id="quota-form"
		method="POST"
		action="?/quota"
		class="flex flex-col gap-4"
		use:enhance={() =>
			async ({ result, update }) => {
				await update({ reset: false });
				const said =
					result.type === 'success' || result.type === 'failure' ? result.data : undefined;
				const message = (said as { message?: string } | undefined)?.message;
				toast.push({
					title: message ?? 'Done.',
					tone: result.type === 'success' ? 'success' : 'danger'
				});
				if (result.type === 'success') open = false;
			}}
	>
		<input type="hidden" name="orgId" value={data.provider} />
		<input type="hidden" name="periodStart" value={data.period.from} />
		<input type="hidden" name="months" value={quotaMonths} />
		<Field label="Salesperson" for="quota-user">
			<Select id="quota-user" name="userId" bind:value={who} options={data.people} />
		</Field>
		<Field label="First-year value to win" for="quota-amount" hint="Empty removes the quota">
			<Input id="quota-amount" name="amount" inputmode="decimal" bind:value={amount} />
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (open = false)}>Cancel</Button>
		<Button type="submit" form="quota-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>
