<!-- Expense entries: what, how much, flags, the receipt; owners keep
     receipts on and delete their drafts. -->
<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Paperclip, Trash2 } from '@lucide/svelte';
	import { Badge, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import { EXPENSE_STATUSES, FLAGS, expenseTone, money } from '#lib/expenses/labels.ts';

	let {
		entries,
		me,
		showWho = false,
		showWhere = false
	}: {
		entries: components['schemas']['Expense'][];
		me: string;
		showWho?: boolean;
		showWhere?: boolean;
	} = $props();

	const statusLabel = (s: string) => EXPENSE_STATUSES.find((x) => x.value === s)?.label ?? s;
	const said: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update();
			const back = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (back as { message?: string } | undefined)?.message;
			toast.push({
				title: message ?? 'Done.',
				tone: result.type === 'success' ? 'success' : 'danger'
			});
		};
	const draft = (e: { status: string; userId: string }) =>
		e.userId === me && (e.status === 'draft' || e.status === 'rejected');
</script>

<ul class="flex flex-col divide-y divide-border text-sm" aria-label="Expenses">
	{#each entries as e (e.id)}
		<li class="flex flex-wrap items-start justify-between gap-3 py-2 first:pt-0">
			<div class="min-w-0">
				<p class="font-medium text-fg">
					{e.description}
					<span class="font-normal text-fg-muted">· {e.categoryName}</span>
				</p>
				<p class="text-xs text-fg-subtle">
					{e.spentOn}{e.vendor ? ` · ${e.vendor}` : ''}{e.distance
						? ` · ${Number(e.distance)} ${e.unit} at ${money(e.rate ?? 0, e.currency)}`
						: ''}{showWho ? ` · ${e.userName}` : ''}{showWhere
						? ` · ${e.ticketRef ?? e.projectName ?? e.orgName}`
						: ''}
				</p>
				<p class="mt-1 flex flex-wrap gap-1">
					<Badge tone={expenseTone(e.status)}>{statusLabel(e.status)}</Badge>
					{#if e.billable}<Badge tone="info">Billable {money(e.billAmount, e.currency)}</Badge>{/if}
					{#if e.reimbursable}<Badge>Pay back</Badge>{/if}
					{#each e.flags as f (f)}<Badge tone="warning">{FLAGS[f] ?? f}</Badge>{/each}
				</p>
			</div>
			<div class="flex items-center gap-2">
				<span class="font-mono text-fg tabular-nums">{money(e.amount, e.currency)}</span>
				{#if e.hasReceipt}
					<a
						href={resolve('/(app)/expenses/[id]/receipt', { id: e.id })}
						target="_blank"
						class="text-accent hover:underline"
						aria-label="Receipt for {e.description}"
					>
						<Paperclip size={14} />
					</a>
				{/if}
				{#if draft(e)}
					{#if !e.hasReceipt && e.kind === 'expense'}
						<form
							method="POST"
							action="?/uploadReceipt"
							enctype="multipart/form-data"
							use:enhance={said}
						>
							<input type="hidden" name="id" value={e.id} />
							<label class="cursor-pointer text-xs text-accent hover:underline">
								Add receipt
								<input
									type="file"
									name="receipt"
									class="sr-only"
									accept="image/jpeg,image/png,image/gif,image/webp,image/heic,application/pdf"
									aria-label="Receipt for {e.description}"
									onchange={(ev) => ev.currentTarget.form?.requestSubmit()}
								/>
							</label>
						</form>
					{/if}
					<form method="POST" action="?/deleteExpense" use:enhance={said}>
						<input type="hidden" name="id" value={e.id} />
						<button
							type="submit"
							class="grid size-7 place-items-center text-fg-subtle hover:text-danger"
							aria-label="Delete {e.description}"
						>
							<Trash2 size={14} />
						</button>
					</form>
				{/if}
			</div>
		</li>
	{:else}
		<li class="py-1 text-fg-muted">None yet.</li>
	{/each}
</ul>
