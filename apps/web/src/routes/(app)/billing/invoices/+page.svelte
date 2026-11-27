<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Search } from '@lucide/svelte';
	import { Badge, Button, EmptyState, Input, Select } from '@usehuxley/ui';
	import { INVOICE_STATUSES, invRef, invoiceTone, money } from '#lib/billing/labels.ts';
	import { REVIEW_STATUSES, label as labelOf, reviewTone } from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	const label = (s: string) => INVOICE_STATUSES.find((x) => x.value === s)?.label ?? s;
</script>

<svelte:head>
	<title>Invoices · Billing · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div>
		<a href={resolve('/(app)/billing')} class="text-sm text-fg-muted hover:text-fg">← Billing</a>
		<h1 class="mt-1 text-[26px] leading-tight font-extrabold tracking-tight text-fg">Invoices</h1>
		{#if data.runId}<p class="mt-1 text-sm text-fg-subtle">From one billing run.</p>{/if}
	</div>

	{#if data.runId && data.canApprove && data.invoices.some((i) => i.status === 'draft')}
		<form method="POST" action="?/approveRun" use:enhance class="flex flex-wrap items-center gap-3">
			<input type="hidden" name="runId" value={data.runId} />
			<Button type="submit" variant="primary">Approve the run</Button>
			<label class="flex items-center gap-2 text-sm text-fg">
				<input type="checkbox" name="submitReviews" checked /> Submit those that need review
			</label>
		</form>
	{/if}
	{#if form && 'approval' in form && form.approval}
		<section
			class="rounded-card border border-border bg-surface p-4 text-sm"
			aria-label="Run approval"
		>
			<p class="font-semibold text-fg">
				{form.approval.approved.length} approved{form.approval.exceptions.length
					? `, ${form.approval.exceptions.length} left out`
					: ''}.
			</p>
			{#if form.approval.exceptions.length}
				<ul class="mt-2 flex flex-col gap-1">
					{#each form.approval.exceptions as x (x.invoiceId)}
						<li>
							<a
								href={resolve('/(app)/billing/invoices/[id]', { id: x.invoiceId })}
								class="font-semibold text-accent hover:underline"
							>
								{invRef(x.number)}
							</a>
							{x.orgName}: {x.reason}
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	{/if}
	{#if form && 'message' in form && form.message}
		<p class="text-sm text-danger" role="alert">{form.message}</p>
	{/if}

	<form method="GET" class="flex flex-wrap items-center gap-2" role="search">
		{#if data.runId}<input type="hidden" name="runId" value={data.runId} />{/if}
		<label for="status" class="sr-only">Status</label>
		<Select
			id="status"
			name="status"
			value={data.status}
			options={[{ value: '', label: 'Any status' }, ...INVOICE_STATUSES.map((s) => ({ ...s }))]}
		/>
		<label for="q" class="sr-only">Search invoices</label>
		<Input id="q" name="q" value={data.q} placeholder="INV number or client" class="w-56" />
		<Button type="submit"><Search size={16} /> Search</Button>
	</form>

	{#if data.invoices.length === 0}
		<EmptyState
			title="No invoices"
			description="Run billing to draft invoices from billable work."
		/>
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full text-left text-sm" aria-label="Invoices">
				<thead class="border-b border-border text-xs font-bold text-fg-subtle">
					<tr>
						<th scope="col" class="px-4 py-2.5">Invoice</th>
						<th scope="col" class="px-4 py-2.5">Client</th>
						<th scope="col" class="px-4 py-2.5">Status</th>
						<th scope="col" class="px-4 py-2.5">Period</th>
						<th scope="col" class="px-4 py-2.5 text-right">Total</th>
					</tr>
				</thead>
				<tbody>
					{#each data.invoices as i (i.id)}
						<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
							<td class="px-4 py-2.5">
								<a
									href={resolve('/(app)/billing/invoices/[id]', { id: i.id })}
									class="font-semibold text-fg hover:text-accent"
								>
									{invRef(i.number)}
								</a>
								{#if i.kind === 'credit_memo'}<Badge>Credit memo</Badge>{/if}
							</td>
							<td class="px-4 py-2.5 text-fg-muted">{i.orgName}</td>
							<td class="px-4 py-2.5">
								<Badge tone={invoiceTone(i.status)}>{label(i.status)}</Badge>
								{#if i.onHold}<Badge tone="warning">On hold</Badge>{/if}
								{#if i.status === 'draft' && i.reviewStatus !== 'not_needed'}
									<Badge tone={reviewTone(i.reviewStatus)}>
										{labelOf(REVIEW_STATUSES, i.reviewStatus)}
									</Badge>
								{/if}
							</td>
							<td class="px-4 py-2.5 text-fg-muted">
								{i.periodStart ?? ''}{i.periodEnd ? ` to ${i.periodEnd}` : ''}
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(i.total, i.currency)}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>
