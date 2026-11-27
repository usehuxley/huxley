<script lang="ts">
	import { resolve } from '$app/paths';
	import { Badge, Card, EmptyState } from '@usehuxley/ui';
	import { REPORT_STATUSES, reportTone } from '#lib/expenses/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	const label = (s: string) => REPORT_STATUSES.find((x) => x.value === s)?.label ?? s;
</script>

<svelte:head>
	<title>Expense reports · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div>
		<a href={resolve('/(app)/expenses')} class="text-sm text-fg-muted hover:text-fg">← Expenses</a>
		<h1 class="mt-1 text-[26px] leading-tight font-extrabold tracking-tight text-fg">
			Expense reports
		</h1>
	</div>
	<Card>
		{#if data.reports.length === 0}
			<EmptyState title="No reports" description="Submit expenses from the Expenses page." />
		{:else}
			<ul class="flex flex-col divide-y divide-border text-sm" aria-label="Expense reports">
				{#each data.reports as r (r.id)}
					<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/expenses/reports/[id]', { id: r.id })}
								class="font-semibold text-fg hover:text-accent"
							>
								EXP-{r.number}
								{r.title}
							</a>
							<p class="text-xs text-fg-subtle">
								{r.userId === data.me ? 'You' : r.userName} · {new Date(
									r.submittedAt
								).toLocaleDateString()}
							</p>
						</div>
						<Badge tone={reportTone(r.status)}>{label(r.status)}</Badge>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
