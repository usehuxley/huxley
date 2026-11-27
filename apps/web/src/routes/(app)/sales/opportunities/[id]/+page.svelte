<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Trophy, XCircle } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast, tray } from '@usehuxley/ui';
	import OpportunityFields from '#lib/sales/OpportunityFields.svelte';
	import {
		FORECASTS,
		QUOTE_STATUSES,
		label,
		money,
		oppRef,
		quoteRef,
		quoteTone,
		statusTone
	} from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let o = $derived(data.opportunity);
	let open = $derived(data.stages.filter((s) => s.kind === 'open'));
	let stageIndex = $derived(open.findIndex((s) => s.id === o.stageId));
	let stageProbability = $derived(data.stages.find((s) => s.id === o.stageId)?.probability);
	let saveErrors = $derived((form && 'save' in form ? form.save?.errors : undefined) ?? {});

	$effect(() => {
		tray.open({
			key: `opportunity:${o.id}`,
			kind: 'other',
			href: `/sales/opportunities/${o.id}`,
			title: o.name,
			subtitle: `${oppRef(o.number)} · ${o.orgName}`
		});
	});

	let closing = $state<'won' | 'lost' | null>(null);
	let closeOpen = $state(false);
	let closeStage = $derived(data.stages.find((s) => s.kind === closing));

	const done =
		(message: string, after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failed = Object.values(result.data ?? {})[0] as { message?: string } | undefined;
				if (failed?.message) toast.push({ title: failed.message, tone: 'danger' });
			}
		};
	const when = (iso: string) => new Date(iso).toLocaleString();
</script>

<svelte:head>
	<title>{o.name} · Sales · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<div>
		<a href={resolve('/(app)/sales')} class="text-sm text-fg-muted hover:text-fg">← Pipeline</a>
		<div class="mt-2 flex flex-wrap items-start justify-between gap-3">
			<div class="min-w-0">
				<p class="font-mono text-[12.5px] text-fg-subtle">
					{oppRef(o.number)} ·
					<a
						href={resolve('/(app)/clients/[id]', { id: o.orgId })}
						class="hover:text-fg hover:underline"
					>
						{o.orgName}
					</a>
				</p>
				<h1 class="mt-1 text-2xl font-extrabold tracking-tight text-balance text-fg">{o.name}</h1>
				<div class="mt-2 flex flex-wrap items-center gap-2">
					<Badge tone={statusTone(o.status)}>{o.stageName}</Badge>
					<Badge>{o.probability}% likely</Badge>
					<Badge>{label(FORECASTS, o.forecast)}</Badge>
					{#if o.reasonName}<Badge tone={statusTone(o.status)}>{o.reasonName}</Badge>{/if}
				</div>
			</div>
			{#if data.canWrite && o.status === 'open'}
				<div class="flex gap-2">
					<Button
						onclick={() => {
							closing = 'won';
							closeOpen = true;
						}}
					>
						<Trophy size={16} /> Won
					</Button>
					<Button
						onclick={() => {
							closing = 'lost';
							closeOpen = true;
						}}
					>
						<XCircle size={16} /> Lost
					</Button>
				</div>
			{/if}
		</div>

		<!-- The open stages; choosing one moves it there. -->
		<form method="POST" action="?/move" class="mt-4 max-w-4xl" use:enhance={done('Moved.')}>
			<input type="hidden" name="version" value={o.version} />
			<div
				class="grid gap-[3px]"
				style="grid-template-columns: repeat({open.length}, minmax(0, 1fr))"
				role="group"
				aria-label="Stage"
			>
				{#each open as s, i (s.id)}
					<button
						type="submit"
						name="stageId"
						value={s.id}
						disabled={!data.canWrite || s.id === o.stageId}
						aria-current={s.id === o.stageId ? 'step' : undefined}
						class="truncate px-2 py-1.5 text-left text-[11px] font-bold tracking-[0.04em] uppercase first:rounded-l-md last:rounded-r-md disabled:cursor-default {o.status !==
						'open'
							? 'bg-surface-sunken text-fg-subtle'
							: s.id === o.stageId
								? 'bg-accent text-accent-fg'
								: i < stageIndex
									? 'bg-fg-muted text-surface hover:opacity-90'
									: 'bg-surface-sunken text-fg-subtle hover:bg-border'}"
						title="{s.name}: {s.probability}%"
					>
						{s.name}
					</button>
				{/each}
			</div>
		</form>
	</div>

	<div class="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem]">
		<Card title="Details">
			<form method="POST" action="?/save" class="flex flex-col gap-4" use:enhance={done('Saved.')}>
				<input type="hidden" name="version" value={o.version} />
				<fieldset disabled={!data.canWrite} class="flex flex-col gap-4">
					<OpportunityFields
						values={o}
						people={data.people}
						contacts={data.contacts}
						errors={saveErrors}
						{stageProbability}
					/>
				</fieldset>
				{#if data.canWrite}
					<div class="flex justify-between gap-2">
						<Button type="submit" variant="primary">Save</Button>
						<Button type="submit" formaction="?/delete" variant="ghost">Delete</Button>
					</div>
				{/if}
			</form>
		</Card>

		<div class="flex flex-col gap-6">
			<Card title="Value">
				<dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
					<dt class="text-fg-muted">One-time</dt>
					<dd class="text-right font-mono tabular-nums">{money(o.amount, o.currency)}</dd>
					<dt class="text-fg-muted">Monthly</dt>
					<dd class="text-right font-mono tabular-nums">{money(o.recurring, o.currency)}</dd>
					<dt class="text-fg-muted">First year</dt>
					<dd class="text-right font-mono tabular-nums">
						{money(Number(o.amount) + 12 * Number(o.recurring), o.currency)}
					</dd>
					<dt class="font-semibold text-fg">Weighted</dt>
					<dd class="text-right font-mono font-semibold tabular-nums">
						{money(o.weighted, o.currency)}
					</dd>
				</dl>
			</Card>
			<Card title="Quotes">
				{#snippet actions()}
					{#if data.canWrite}
						<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- resolve() plus a query string -->
						<a
							href="{resolve('/(app)/sales/quotes/new')}?opportunity={o.id}"
							class="text-sm font-semibold text-accent hover:underline"
						>
							New quote
						</a>
					{/if}
				{/snippet}
				{#if data.quotes.length === 0}
					<p class="text-sm text-fg-muted">No quotes yet.</p>
				{:else}
					<ul class="flex flex-col gap-2 text-sm" aria-label="Quotes">
						{#each data.quotes as qt (qt.id)}
							<li class="flex items-start justify-between gap-2">
								<span class="min-w-0">
									<a
										href={resolve('/(app)/sales/quotes/[id]', { id: qt.id })}
										class="font-semibold text-fg hover:text-accent"
									>
										{qt.title}
									</a>
									<span class="block font-mono text-xs text-fg-subtle">
										{quoteRef(qt.number)} · rev {qt.revision} · {money(qt.oneTime, qt.currency)} + {money(
											qt.monthly,
											qt.currency
										)}/mo
									</span>
								</span>
								<Badge tone={quoteTone(qt.status)}>{label(QUOTE_STATUSES, qt.status)}</Badge>
							</li>
						{/each}
					</ul>
				{/if}
			</Card>
			<Card title="History">
				<ol class="flex flex-col gap-2 text-sm" aria-label="Stage history">
					{#each data.history as h, i (i)}
						<li class="flex flex-col">
							<span class="text-fg">
								{h.fromStageName
									? `${h.fromStageName} → ${h.toStageName}`
									: `Created at ${h.toStageName}`}
							</span>
							<span class="text-xs text-fg-subtle">
								{h.actorName ?? 'Huxley'} · <time datetime={h.at}>{when(h.at)}</time>
							</span>
						</li>
					{/each}
				</ol>
				{#if o.reasonNote}
					<p class="mt-3 rounded-control bg-surface-sunken px-3 py-2 text-sm text-fg">
						{o.reasonNote}
					</p>
				{/if}
			</Card>
		</div>
	</div>
</div>

<Dialog
	bind:open={closeOpen}
	title={closing === 'won' ? 'Mark it won' : 'Mark it lost'}
	description="Say why, so win and loss reports can tell."
>
	<form
		id="close-form"
		method="POST"
		action="?/move"
		class="flex flex-col gap-4"
		use:enhance={done(
			closing === 'won' ? 'Marked won.' : 'Marked lost.',
			() => (closeOpen = false)
		)}
	>
		<input type="hidden" name="version" value={o.version} />
		<input type="hidden" name="stageId" value={closeStage?.id ?? ''} />
		<Field label="Reason" for="reasonId">
			<Select
				id="reasonId"
				name="reasonId"
				options={[
					{ value: '', label: 'Choose…' },
					...data.reasons
						.filter((r) => r.kind === closing)
						.map((r) => ({ value: r.id, label: r.name }))
				]}
			/>
		</Field>
		<Field label="Note" for="reasonNote">
			<Input id="reasonNote" name="reasonNote" maxlength={2000} />
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (closeOpen = false)}>Cancel</Button>
		<Button type="submit" form="close-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>
