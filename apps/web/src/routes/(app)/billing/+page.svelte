<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Receipt, Search } from '@lucide/svelte';
	import { Badge, Button, Card, EmptyState, Input, toast } from '@usehuxley/ui';
	import { money } from '#lib/billing/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let picked = $state<string[]>([]);
	let total = $derived(data.unbilled.reduce((t, u) => t + Number(u.total), 0));
	let currency = $derived(data.unbilled[0]?.currency ?? 'USD');
</script>

<svelte:head>
	<title>Billing · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div class="flex flex-wrap items-end gap-4">
		<div>
			<h1 class="text-[26px] leading-tight font-extrabold tracking-tight text-fg">Billing</h1>
			<p class="mt-1 text-sm text-fg-subtle">
				What's ready to bill, by client, and the runs that billed it.
			</p>
		</div>
		<div class="ml-auto flex gap-2">
			<Button href={resolve('/(app)/billing/accounting')}>
				Accounting{#if data.syncErrors}<Badge tone="danger">{data.syncErrors}</Badge>{/if}
			</Button>
			<Button href={resolve('/(app)/billing/invoices')}><Receipt size={16} /> Invoices</Button>
		</div>
	</div>

	<form method="GET" class="flex flex-wrap items-end gap-2">
		<input type="hidden" name="org" value={data.orgId} />
		<label class="text-sm text-fg">
			From <Input name="from" type="date" value={data.from} class="w-40" />
		</label>
		<label class="text-sm text-fg">
			To <Input name="to" type="date" value={data.to} class="w-40" />
		</label>
		<Button type="submit"><Search size={16} /> Show</Button>
	</form>

	{#if data.unbilled.length === 0}
		<EmptyState
			title="Nothing to bill"
			description="No billable time, deliveries, recurring charges or project fees in this period."
		/>
	{:else}
		<form
			method="POST"
			action="?/run"
			use:enhance={() =>
				async ({ result, update }) => {
					if (result.type === 'failure') {
						const failed = result.data as { message?: string } | undefined;
						toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
					}
					await update({ reset: false });
				}}
		>
			<input type="hidden" name="orgId" value={data.orgId} />
			<input type="hidden" name="from" value={data.from} />
			<input type="hidden" name="to" value={data.to} />
			<section class="overflow-x-auto rounded-card border border-border bg-surface">
				<table class="w-full text-left text-sm" aria-label="Unbilled">
					<thead class="border-b border-border text-xs font-bold text-fg-subtle">
						<tr>
							<th scope="col" class="w-10 px-3 py-2.5"><span class="sr-only">Bill</span></th>
							<th scope="col" class="px-3 py-2.5">Client</th>
							<th scope="col" class="px-3 py-2.5 text-right">Time</th>
							<th scope="col" class="px-3 py-2.5 text-right">Products</th>
							<th scope="col" class="px-3 py-2.5 text-right">Recurring</th>
							<th scope="col" class="px-3 py-2.5 text-right">Projects</th>
							<th scope="col" class="px-3 py-2.5 text-right">Total</th>
						</tr>
					</thead>
					<tbody>
						{#each data.unbilled as u (`${u.orgId}-${u.currency}`)}
							<tr class="border-b border-border last:border-0">
								<td class="px-3 py-2.5">
									<input
										type="checkbox"
										name="clients"
										value={u.orgId}
										bind:group={picked}
										aria-label="Bill {u.orgName}"
									/>
								</td>
								<td class="px-3 py-2.5 font-semibold text-fg">{u.orgName}</td>
								<td class="px-3 py-2.5 text-right font-mono tabular-nums">
									{money(u.time, u.currency)}
								</td>
								<td class="px-3 py-2.5 text-right font-mono tabular-nums">
									{money(u.products, u.currency)}
								</td>
								<td class="px-3 py-2.5 text-right font-mono tabular-nums">
									{money(u.recurring, u.currency)}
								</td>
								<td class="px-3 py-2.5 text-right font-mono tabular-nums">
									{money(u.projects, u.currency)}
								</td>
								<td class="px-3 py-2.5 text-right font-mono font-semibold tabular-nums">
									{money(u.total, u.currency)}
								</td>
							</tr>
						{/each}
					</tbody>
					<tfoot>
						<tr class="border-t border-border">
							<th scope="row" colspan="6" class="px-3 py-2.5 text-right font-semibold text-fg">
								Total
							</th>
							<td class="px-3 py-2.5 text-right font-mono font-semibold tabular-nums">
								{money(total, currency)}
							</td>
						</tr>
					</tfoot>
				</table>
			</section>
			{#if data.canWrite}
				<div class="mt-3 flex items-center gap-3">
					<Button type="submit" variant="primary">
						{picked.length
							? `Bill ${picked.length} client${picked.length === 1 ? '' : 's'}`
							: 'Bill everyone'}
					</Button>
					<span class="text-sm text-fg-subtle">
						Draft invoices are made for review; nothing is sent yet.
					</span>
				</div>
			{/if}
		</form>
	{/if}

	{#if data.runs.length}
		<Card title="Billing runs">
			<ul class="flex flex-col divide-y divide-border text-sm" aria-label="Billing runs">
				{#each data.runs as r (r.id)}
					<li class="flex items-center justify-between py-2">
						<span>
							{r.periodStart} to {r.periodEnd}
							<span class="text-fg-subtle">
								· {r.invoices} invoice{r.invoices === 1 ? '' : 's'}{r.createdByName
									? ` · ${r.createdByName}`
									: ''}
							</span>
						</span>
						<a
							href="{resolve('/(app)/billing/invoices')}?runId={r.id}"
							class="text-accent hover:underline"
						>
							Invoices
						</a>
					</li>
				{/each}
			</ul>
		</Card>
	{/if}
</div>
