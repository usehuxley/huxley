<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, toast } from '@usehuxley/ui';
	import ExpenseList from '#lib/expenses/ExpenseList.svelte';
	import { FLAGS, REPORT_STATUSES, money, reportTone } from '#lib/expenses/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let r = $derived(data.report);
	const label = (s: string) => REPORT_STATUSES.find((x) => x.value === s)?.label ?? s;
	const said =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update();
			const back = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (back as { message?: string } | undefined)?.message;
			toast.push({
				title: message ?? 'Done.',
				tone: result.type === 'success' ? 'success' : 'danger'
			});
			if (result.type === 'success') after?.();
		};
	let body = $state('');
</script>

<svelte:head>
	<title>EXP-{r.number} · Expense reports · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-5">
	<div>
		<a href={resolve('/(app)/expenses/reports')} class="text-sm text-fg-muted hover:text-fg">
			← Expense reports
		</a>
		<p class="mt-2 font-mono text-[12.5px] text-fg-subtle">EXP-{r.number} · {r.userName}</p>
		<h1 class="mt-1 text-2xl font-extrabold tracking-tight text-fg">{r.title}</h1>
		<div class="mt-2 flex flex-wrap items-center gap-2">
			<Badge tone={reportTone(r.status)}>{label(r.status)}</Badge>
			{#each r.totals as t (t.currency)}
				<Badge>
					{money(t.amount, t.currency)}{Number(t.reimbursable) > 0
						? ` · ${money(t.reimbursable, t.currency)} to pay back`
						: ''}
				</Badge>
			{/each}
			{#each Object.entries(r.flags) as [f, n] (f)}
				<Badge tone="warning">{n} {FLAGS[f]?.toLowerCase() ?? f}</Badge>
			{/each}
		</div>
		{#if r.decidedBy}
			<p class="mt-2 text-sm text-fg-muted">
				{label(r.status)} by {r.decidedBy}{r.comment ? `: “${r.comment}”` : ''}
			</p>
		{/if}
	</div>

	{#if data.approval}
		<Card title="Your decision">
			<form method="POST" action="?/decideReview" class="flex flex-col gap-2" use:enhance={said()}>
				<input type="hidden" name="id" value={data.approval.id} />
				<textarea
					name="comment"
					rows="2"
					maxlength="2000"
					aria-label="Decision comment"
					placeholder="What should change, or why (optional)"
					class="rounded-control border border-border bg-surface p-2 text-sm text-fg"></textarea>
				<div class="flex flex-wrap gap-2">
					<Button type="submit" size="sm" name="decision" value="approve" variant="primary">
						Approve
					</Button>
					<Button type="submit" size="sm" name="decision" value="request_changes">
						Ask for changes
					</Button>
					<Button type="submit" size="sm" name="decision" value="reject" variant="danger">
						Reject
					</Button>
				</div>
			</form>
		</Card>
	{/if}

	{#if data.sync && data.canPay && (r.status === 'approved' || r.status === 'reimbursed')}
		<Card title="Paying it back">
			<div class="flex flex-wrap items-center justify-between gap-3 text-sm">
				<div class="min-w-0">
					{#if r.status === 'reimbursed'}
						<p class="text-fg">Paid back.</p>
					{/if}
					{#if data.sync.syncStatus === 'synced'}
						<p class="text-fg-muted">
							In {data.sync.syncTarget} as a bill{data.sync.remoteNumber
								? ` (${data.sync.remoteNumber})`
								: ''}.
							{#if data.sync.remoteUrl}
								<!-- eslint-disable svelte/no-navigation-without-resolve -- the accounting system's page -->
								<a
									href={data.sync.remoteUrl}
									target="_blank"
									rel="noopener noreferrer"
									class="text-accent hover:underline"
								>
									Open it
								</a>
								<!-- eslint-enable svelte/no-navigation-without-resolve -->
							{/if}
						</p>
					{:else if data.sync.syncStatus === 'pending'}
						<p class="text-fg-muted">Being sent to {data.sync.syncTarget}…</p>
					{:else if data.sync.syncStatus === 'failed'}
						<p class="text-xs text-danger">{data.sync.syncError}</p>
					{/if}
				</div>
				{#if r.status === 'approved'}
					<div class="flex gap-2">
						{#if data.sync.syncStatus !== 'synced'}
							<form method="POST" action="?/sendToAccounting" use:enhance={said()}>
								<Button type="submit" size="sm">
									{data.sync.syncStatus === 'failed' ? 'Try again' : 'Send to accounting'}
								</Button>
							</form>
						{/if}
						<form method="POST" action="?/reimbursed" use:enhance={said()}>
							<Button type="submit" size="sm" variant="primary">Mark paid back</Button>
						</form>
					</div>
				{/if}
			</div>
		</Card>
	{/if}

	<Card title="Expenses">
		<ExpenseList entries={r.entries} me={data.me} showWhere />
	</Card>

	<Card title="Comments">
		<ul class="flex flex-col gap-3 text-sm" aria-label="Comments">
			{#each data.comments as c (c.id)}
				<li>
					<p class="text-xs text-fg-subtle">
						<span class="font-semibold text-fg">{c.authorName ?? 'Someone'}</span> · {new Date(
							c.createdAt
						).toLocaleString()}
					</p>
					<p class="whitespace-pre-wrap text-fg">{c.body}</p>
				</li>
			{:else}
				<li class="text-fg-muted">No comments.</li>
			{/each}
		</ul>
		<form
			method="POST"
			action="?/comment"
			class="mt-3 flex flex-col gap-2"
			use:enhance={said(() => (body = ''))}
		>
			<textarea
				name="body"
				rows="2"
				required
				maxlength="4000"
				aria-label="Add a comment"
				bind:value={body}
				class="rounded-control border border-border-strong bg-surface p-2 text-sm text-fg"
			></textarea>
			<div><Button type="submit" size="sm">Comment</Button></div>
		</form>
	</Card>
</div>
