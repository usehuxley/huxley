<script lang="ts">
	import { deserialize, enhance } from '$app/forms';
	import { goto, invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { CalendarClock, LayoutGrid, List, Plus, Search } from '@lucide/svelte';
	import { Badge, Button, Dialog, EmptyState, Field, Input, Select, toast } from '@usehuxley/ui';
	import { money, oppRef, statusTone } from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	type Opportunity = (typeof data.opportunities)[number];
	let open = $derived(data.stages.filter((s) => s.kind === 'open'));
	let closing = $derived(data.stages.filter((s) => s.kind !== 'open'));
	let byStage = $derived(
		new Map(open.map((s) => [s.id, data.opportunities.filter((o) => o.stageId === s.id)]))
	);
	const sum = (list: Opportunity[], key: 'amount' | 'weighted' | 'recurring') =>
		list.reduce((n, o) => n + Number(o[key]), 0);
	let currency = $derived(data.opportunities[0]?.currency ?? 'USD');

	// Moving by drag and drop: open stages move straight away; won and lost ask why.
	let dragging = $state<Opportunity | null>(null);
	let over = $state<string | null>(null);
	let closeTo = $state<{ o: Opportunity; stageId: string; kind: string } | null>(null);
	let closeOpen = $state(false);

	async function move(o: Opportunity, stageId: string) {
		const body = new FormData();
		body.set('id', o.id);
		body.set('stageId', stageId);
		body.set('version', String(o.version));
		const response = await fetch('?/move', {
			method: 'POST',
			body,
			headers: { 'x-sveltekit-action': 'true' }
		});
		const result = deserialize(await response.text());
		if (result.type === 'failure') {
			toast.push({
				title: (result.data as { message?: string })?.message ?? 'Could not move it.',
				tone: 'danger'
			});
		}
		await invalidateAll();
	}

	function drop(stage: { id: string; kind: string }) {
		const o = dragging;
		dragging = null;
		over = null;
		if (!o || o.stageId === stage.id) return;
		if (stage.kind === 'open') void move(o, stage.id);
		else {
			closeTo = { o, stageId: stage.id, kind: stage.kind };
			closeOpen = true;
		}
	}

	function filter(params: Record<string, string>) {
		const url = new URL(page.url.href);
		for (const [k, v] of Object.entries(params)) {
			if (v) url.searchParams.set(k, v);
			else url.searchParams.delete(k);
		}
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- same page, new query
		void goto(`${url.pathname}${url.search}`);
	}

	const day = (d?: string | null) =>
		d
			? new Date(`${d}T00:00:00`).toLocaleDateString(undefined, { day: 'numeric', month: 'short' })
			: '';
	const overdue = (d?: string | null) => !!d && d < new Date().toLocaleDateString('en-CA');
</script>

<svelte:head>
	<title>Pipeline · Sales · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div class="flex flex-wrap items-end gap-4">
		<div class="min-w-0">
			<h1 class="text-[26px] leading-tight font-extrabold tracking-tight text-fg">Pipeline</h1>
			<p class="mt-1 text-sm text-fg-subtle">
				{#if data.view === 'board'}
					{data.opportunities.length} open · {money(sum(data.opportunities, 'amount'), currency)} one-time
					· {money(sum(data.opportunities, 'recurring'), currency)}/mo recurring · weighted {money(
						sum(data.opportunities, 'weighted'),
						currency
					)}
				{:else}
					{data.opportunities.length} opportunit{data.opportunities.length === 1 ? 'y' : 'ies'}
				{/if}
			</p>
		</div>
		<div class="ml-auto flex flex-wrap items-center gap-2">
			<Button href={resolve('/(app)/sales/leads')} variant="secondary">Leads</Button>
			{#if data.canWrite}
				<Button href={resolve('/(app)/sales/opportunities/new')} variant="primary">
					<Plus size={16} /> New opportunity
				</Button>
			{/if}
		</div>
	</div>

	<div class="flex flex-wrap items-center gap-2">
		<div
			class="inline-flex overflow-hidden rounded-control border border-border-strong"
			role="group"
			aria-label="View"
		>
			<button
				type="button"
				class="flex items-center gap-1.5 px-3 py-1.5 text-sm font-semibold {data.view === 'board'
					? 'bg-fg text-surface'
					: 'text-fg-muted hover:bg-surface-sunken'}"
				aria-pressed={data.view === 'board'}
				onclick={() => filter({ view: '', status: '' })}
			>
				<LayoutGrid size={15} /> Board
			</button>
			<button
				type="button"
				class="flex items-center gap-1.5 px-3 py-1.5 text-sm font-semibold {data.view === 'list'
					? 'bg-fg text-surface'
					: 'text-fg-muted hover:bg-surface-sunken'}"
				aria-pressed={data.view === 'list'}
				onclick={() => filter({ view: 'list' })}
			>
				<List size={15} /> List
			</button>
		</div>
		{#if data.view === 'list'}
			<label class="sr-only" for="status">Status</label>
			<Select
				id="status"
				value={data.status}
				options={[
					{ value: '', label: 'Any status' },
					{ value: 'open', label: 'Open' },
					{ value: 'won', label: 'Won' },
					{ value: 'lost', label: 'Lost' }
				]}
				onchange={(e: Event) => filter({ status: (e.currentTarget as HTMLSelectElement).value })}
			/>
		{/if}
		<label class="flex items-center gap-2 text-sm font-medium text-fg">
			<input
				type="checkbox"
				checked={data.mine}
				onchange={(e) => filter({ mine: e.currentTarget.checked ? '1' : '' })}
			/>
			Only mine
		</label>
		<form method="GET" class="ml-auto flex gap-2" role="search">
			{#if data.view === 'list'}<input type="hidden" name="view" value="list" />{/if}
			<label for="q" class="sr-only">Search opportunities</label>
			<Input id="q" name="q" value={data.q} placeholder="Name, client or number" class="w-56" />
			<Button type="submit"><Search size={16} /> Search</Button>
		</form>
	</div>

	{#if data.view === 'board'}
		<div class="flex gap-3 overflow-x-auto pb-2" aria-label="Pipeline board">
			{#each open as stage (stage.id)}
				{@const items = byStage.get(stage.id) ?? []}
				<section
					class="flex min-w-56 flex-1 flex-col rounded-card border bg-surface-sunken/60 {over ===
					stage.id
						? 'border-accent bg-accent-soft'
						: 'border-border'}"
					aria-label="{stage.name}, {items.length}"
					ondragover={(e) => {
						if (!dragging) return;
						e.preventDefault();
						over = stage.id;
					}}
					ondragleave={() => (over = over === stage.id ? null : over)}
					ondrop={(e) => {
						e.preventDefault();
						drop(stage);
					}}
				>
					<header class="flex items-baseline justify-between gap-2 px-3 pt-3 pb-2">
						<h2 class="text-sm font-bold text-fg">
							{stage.name}
							<span class="ml-1 font-mono text-xs font-medium text-fg-subtle">{items.length}</span>
						</h2>
						<span
							class="text-xs text-fg-subtle tabular-nums"
							title="Weighted at {stage.probability}%"
						>
							{money(sum(items, 'weighted'), currency)}
						</span>
					</header>
					<ol class="flex min-h-24 flex-col gap-2 px-2 pb-2">
						{#each items as o (o.id)}
							<li
								draggable={data.canWrite}
								ondragstart={(e) => {
									dragging = o;
									e.dataTransfer?.setData('text/plain', o.id);
								}}
								ondragend={() => {
									dragging = null;
									over = null;
								}}
								class="rounded-control border border-border bg-surface p-2.5 shadow-card {data.canWrite
									? 'cursor-grab active:cursor-grabbing'
									: ''}"
							>
								<a
									href={resolve('/(app)/sales/opportunities/[id]', { id: o.id })}
									class="block text-sm leading-snug font-semibold text-fg hover:text-accent"
								>
									{o.name}
								</a>
								<p class="mt-0.5 truncate text-xs text-fg-subtle">
									{oppRef(o.number)} · {o.orgName}
								</p>
								<div class="mt-2 flex items-center justify-between gap-2 text-xs">
									<span class="font-mono font-semibold text-fg tabular-nums">
										{money(o.amount, o.currency)}{#if Number(o.recurring) > 0}
											<span class="text-fg-subtle"> + {money(o.recurring, o.currency)}/mo</span>
										{/if}
									</span>
									{#if o.closeDate}
										<span
											class="flex items-center gap-1 {overdue(o.closeDate)
												? 'font-semibold text-danger'
												: 'text-fg-subtle'}"
											title="Expected to close {o.closeDate}"
										>
											<CalendarClock size={12} />
											{day(o.closeDate)}
										</span>
									{/if}
								</div>
								{#if o.ownerName}
									<p class="mt-1 text-xs text-fg-subtle">{o.ownerName}</p>
								{/if}
							</li>
						{:else}
							<li class="px-2 py-6 text-center text-xs text-fg-subtle">Nothing here</li>
						{/each}
					</ol>
				</section>
			{/each}
			{#if data.canWrite}
				<div class="flex w-32 flex-none flex-col gap-3">
					{#each closing as stage (stage.id)}
						<div
							role="region"
							aria-label="Drop here to mark it {stage.name}"
							class="grid flex-1 place-items-center rounded-card border-2 border-dashed p-3 text-center text-sm font-semibold {over ===
							stage.id
								? stage.kind === 'won'
									? 'border-success bg-success-soft text-success'
									: 'border-danger bg-danger-soft text-danger'
								: 'border-border-strong text-fg-subtle'}"
							ondragover={(e) => {
								if (!dragging) return;
								e.preventDefault();
								over = stage.id;
							}}
							ondragleave={() => (over = over === stage.id ? null : over)}
							ondrop={(e) => {
								e.preventDefault();
								drop(stage);
							}}
						>
							{stage.name}
						</div>
					{/each}
				</div>
			{/if}
		</div>
	{:else if data.opportunities.length === 0}
		<EmptyState title="No opportunities" description="Opportunities in this view show up here." />
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full text-left text-sm" aria-label="Opportunities">
				<thead class="border-b border-border text-xs font-bold text-fg-subtle">
					<tr>
						<th scope="col" class="px-4 py-2.5">Opportunity</th>
						<th scope="col" class="px-4 py-2.5">Client</th>
						<th scope="col" class="px-4 py-2.5">Stage</th>
						<th scope="col" class="px-4 py-2.5 text-right">One-time</th>
						<th scope="col" class="px-4 py-2.5 text-right">Monthly</th>
						<th scope="col" class="px-4 py-2.5 text-right">Weighted</th>
						<th scope="col" class="px-4 py-2.5">Close</th>
						<th scope="col" class="px-4 py-2.5">Owner</th>
					</tr>
				</thead>
				<tbody>
					{#each data.opportunities as o (o.id)}
						<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
							<td class="px-4 py-2.5">
								<a
									href={resolve('/(app)/sales/opportunities/[id]', { id: o.id })}
									class="font-semibold text-fg hover:text-accent"
								>
									{o.name}
								</a>
								<span class="block font-mono text-xs text-fg-subtle">{oppRef(o.number)}</span>
							</td>
							<td class="px-4 py-2.5 text-fg-muted">{o.orgName}</td>
							<td class="px-4 py-2.5">
								<Badge tone={statusTone(o.status)}>{o.stageName}</Badge>
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(o.amount, o.currency)}
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(o.recurring, o.currency)}
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(o.weighted, o.currency)}
							</td>
							<td
								class="px-4 py-2.5 whitespace-nowrap {o.status === 'open' && overdue(o.closeDate)
									? 'font-semibold text-danger'
									: 'text-fg-muted'}"
							>
								{o.closeDate ?? ''}
							</td>
							<td class="px-4 py-2.5 text-fg-muted">{o.ownerName ?? ''}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>

<Dialog
	bind:open={closeOpen}
	title={closeTo ? `Mark ${closeTo.o.name} ${closeTo.kind}` : ''}
	description="Say why, so win and loss reports can tell."
>
	{#if closeTo}
		<form
			id="close-form"
			method="POST"
			action="?/move"
			class="flex flex-col gap-4"
			use:enhance={() =>
				async ({ result, update }) => {
					await update();
					if (result.type === 'success') {
						closeOpen = false;
						toast.push({ title: `Marked ${closeTo?.kind}.`, tone: 'success' });
					}
				}}
		>
			<input type="hidden" name="id" value={closeTo.o.id} />
			<input type="hidden" name="stageId" value={closeTo.stageId} />
			<input type="hidden" name="version" value={closeTo.o.version} />
			<Field label="Reason" for="reasonId">
				<Select
					id="reasonId"
					name="reasonId"
					options={[
						{ value: '', label: 'Choose…' },
						...data.reasons
							.filter((r) => r.kind === closeTo?.kind)
							.map((r) => ({ value: r.id, label: r.name }))
					]}
				/>
			</Field>
			<Field label="Note" for="reasonNote">
				<Input id="reasonNote" name="reasonNote" maxlength={2000} />
			</Field>
		</form>
	{/if}
	{#snippet footer()}
		<Button onclick={() => (closeOpen = false)}>Cancel</Button>
		<Button type="submit" form="close-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>
