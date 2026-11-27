<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, EmptyState, Select, toast } from '@usehuxley/ui';
	import { money } from '#lib/expenses/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
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
	const tone = (s: string) =>
		s === 'matched' ? 'success' : s === 'ignored' ? 'neutral' : 'warning';
</script>

<svelte:head>
	<title>Card charges · Expenses · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div>
		<a href={resolve('/(app)/expenses')} class="text-sm text-fg-muted hover:text-fg">← Expenses</a>
		<h1 class="mt-1 text-[26px] leading-tight font-extrabold tracking-tight text-fg">
			Card charges
		</h1>
		<p class="mt-1 text-sm text-fg-subtle">
			Company card charges, matched to expenses. Those on your card that match nothing can be logged
			as your expense.
		</p>
	</div>

	{#if data.canImport}
		<Card
			title="Import"
			description="A CSV from your bank with date, description and amount columns (and optionally merchant, currency, card, transaction id, and the cardholder's email). Charges imported before are skipped."
		>
			<form
				method="POST"
				action="?/import"
				enctype="multipart/form-data"
				class="flex flex-wrap items-end gap-2"
				use:enhance={said}
			>
				{#if data.providers.length > 1}
					<Select name="orgId" options={data.providers} aria-label="Whose cards" />
				{:else}
					<input type="hidden" name="orgId" value={data.providers[0]?.value ?? ''} />
				{/if}
				<input
					type="file"
					name="file"
					accept=".csv,text/csv"
					aria-label="Card charges CSV"
					class="text-sm text-fg"
				/>
				<Button type="submit" variant="primary">Import</Button>
			</form>
		</Card>
	{/if}

	<form method="GET" class="flex items-center gap-2">
		<label for="status" class="sr-only">Status</label>
		<Select
			id="status"
			name="status"
			value={data.status}
			options={[
				{ value: '', label: 'All' },
				{ value: 'unmatched', label: 'Unmatched' },
				{ value: 'matched', label: 'Matched' },
				{ value: 'ignored', label: 'Ignored' }
			]}
		/>
		<Button type="submit">Show</Button>
	</form>

	<Card>
		{#if data.cards.length === 0}
			<EmptyState title="No card charges" description="Import them from your bank's CSV." />
		{:else}
			<ul class="flex flex-col divide-y divide-border text-sm" aria-label="Card charges">
				{#each data.cards as t (t.id)}
					<li class="flex flex-wrap items-center justify-between gap-3 py-2 first:pt-0">
						<div class="min-w-0">
							<p class="font-medium text-fg">{t.description}</p>
							<p class="text-xs text-fg-subtle">
								{t.postedOn}{t.merchant ? ` · ${t.merchant}` : ''}{t.cardLast4
									? ` · card ${t.cardLast4}`
									: ''}{t.userName ? ` · ${t.userName}` : ''}{t.expenseDescription
									? ` · matched to “${t.expenseDescription}”`
									: ''}
							</p>
						</div>
						<div class="flex flex-wrap items-center gap-2">
							<span class="font-mono text-fg tabular-nums">{money(t.amount, t.currency)}</span>
							<Badge tone={tone(t.status)}>
								{t.status === 'unmatched'
									? 'Unmatched'
									: t.status === 'matched'
										? 'Matched'
										: 'Ignored'}
							</Badge>
							{#if t.status === 'unmatched' && t.userId === data.me && data.categories.length}
								<form
									method="POST"
									action="?/log"
									class="flex items-center gap-1"
									use:enhance={said}
								>
									<input type="hidden" name="id" value={t.id} />
									<Select
										name="categoryId"
										options={data.categories.map((c) => ({ value: c.id, label: c.name }))}
										aria-label="Category for {t.description}"
									/>
									<Button type="submit" size="sm">Log as my expense</Button>
								</form>
							{/if}
							{#if t.status === 'unmatched'}
								<form method="POST" action="?/status" use:enhance={said}>
									<input type="hidden" name="id" value={t.id} />
									<input type="hidden" name="ignored" value="yes" />
									<Button type="submit" size="sm" variant="ghost">Ignore</Button>
								</form>
							{:else}
								<form method="POST" action="?/status" use:enhance={said}>
									<input type="hidden" name="id" value={t.id} />
									<input type="hidden" name="ignored" value="no" />
									<Button type="submit" size="sm" variant="ghost">
										{t.status === 'matched' ? 'Unmatch' : 'Restore'}
									</Button>
								</form>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
