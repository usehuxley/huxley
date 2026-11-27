<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { RefreshCw } from '@lucide/svelte';
	import { Badge, Button, Card, EmptyState, toast } from '@usehuxley/ui';
	import { invRef } from '#lib/billing/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	type Kind = 'customer' | 'item' | 'tax_code' | 'class';
	const TABS: { kind: Kind; label: string }[] = [
		{ kind: 'customer', label: 'Customers' },
		{ kind: 'item', label: 'Items' },
		{ kind: 'tax_code', label: 'Tax codes' },
		{ kind: 'class', label: 'Classes' }
	];
	let tab = $state<Kind>('customer');
	let taxCode = $state('');
	/** Tax codes added here, until they're matched. */
	let extraTax = $state<string[]>([]);

	/** What can be matched on each tab: [local key, what it is]. */
	let locals = $derived.by((): Record<Kind, { key: string; name: string }[]> => ({
		customer: data.clients.map((c) => ({ key: c.id, name: c.name })),
		item: [
			{ key: 'default', name: 'Anything else (the default item)' },
			...['time', 'recurring', 'project', 'product', 'manual'].map((k) => ({
				key: `kind:${k}`,
				name: `${k.charAt(0).toUpperCase()}${k.slice(1)} lines`
			})),
			...data.products.map((p) => ({ key: `product:${p.id}`, name: `${p.name} (${p.sku})` }))
		],
		tax_code: [
			{ key: 'default', name: 'Lines without a tax code' },
			...data.mappings
				.filter((m) => m.kind === 'tax_code' && m.localKey !== 'default')
				.map((m) => ({ key: m.localKey, name: m.localKey })),
			...extraTax
				.filter((t) => !data.mappings.some((m) => m.kind === 'tax_code' && m.localKey === t))
				.map((t) => ({ key: t, name: t }))
		],
		class: [
			{ key: 'default', name: 'Every client (the default class)' },
			...data.clients.map((c) => ({ key: c.id, name: c.name }))
		]
	}));
	const matchOf = (kind: Kind, key: string) =>
		data.mappings.find((m) => m.kind === kind && m.localKey === key);
	let unmatched = $derived(data.clients.filter((c) => !matchOf('customer', c.id)).length);

	const said =
		(): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			const back = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (back as { message?: string } | undefined)?.message;
			toast.push({
				title: message ?? 'Done.',
				tone: result.type === 'success' ? 'success' : 'danger'
			});
		};
	const select = 'h-8 w-full rounded-control border border-border bg-surface px-2 text-sm text-fg';
</script>

<svelte:head>
	<title>Accounting · Billing · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div>
		<a href={resolve('/(app)/billing')} class="text-sm text-fg-muted hover:text-fg">← Billing</a>
		<h1 class="mt-1 text-[26px] leading-tight font-extrabold tracking-tight text-fg">Accounting</h1>
		<p class="mt-1 text-sm text-fg-subtle">
			{#if data.target}
				Approved invoices sync to <span class="font-semibold text-fg">{data.target}</span>{data
					.settings?.onApproval
					? ' as soon as they are approved'
					: ' when you sync them'}{data.settings?.enabled ? '' : ' (turned off)'}.
			{:else}
				Choose the accounting system in Settings → Billing, and install its pack.
			{/if}
		</p>
	</div>

	{#if data.queue.length}
		<Card
			title="Sync errors"
			description="Invoices the accounting system didn't take. Fix the cause, then retry."
		>
			<ul class="flex flex-col divide-y divide-border text-sm" aria-label="Sync errors">
				{#each data.queue as q (q.invoiceId)}
					<li class="flex items-start justify-between gap-3 py-2">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/billing/invoices/[id]', { id: q.invoiceId })}
								class="font-semibold text-fg hover:text-accent"
							>
								{invRef(q.number)}
							</a>
							<span class="text-fg-muted">
								· {q.orgName} · {q.attempts} attempt{q.attempts === 1 ? '' : 's'}
							</span>
							<p class="text-xs text-danger">{q.error}</p>
						</div>
						{#if data.canWrite}
							<form method="POST" action="?/retry" use:enhance={said()}>
								<input type="hidden" name="invoiceId" value={q.invoiceId} />
								<Button type="submit" size="sm"><RefreshCw size={14} /> Retry</Button>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	{#if data.target}
		<Card title="Matching">
			<p class="mb-3 text-sm text-fg-muted">
				Match clients to customers, products and kinds of line to items, and tax codes and classes.
				The lists come from the pack's Fetch lists workflow.
				{#if unmatched}<Badge tone="warning">
						{unmatched} client{unmatched === 1 ? '' : 's'} not matched
					</Badge>{/if}
			</p>
			<nav class="mb-3 flex gap-1 border-b border-border" aria-label="What to match">
				{#each TABS as t (t.kind)}
					<button
						type="button"
						onclick={() => (tab = t.kind)}
						aria-current={tab === t.kind ? 'page' : undefined}
						class="-mb-px border-b-2 px-3 py-1.5 text-sm {tab === t.kind
							? 'border-accent font-medium text-fg'
							: 'border-transparent text-fg-muted hover:text-fg'}"
					>
						{t.label}
					</button>
				{/each}
			</nav>
			{#if locals[tab].length === 0}
				<EmptyState title="Nothing to match" description="There's nothing of this kind yet." />
			{:else}
				<table class="w-full text-left text-sm" aria-label="Matches">
					<thead class="border-b border-border text-xs font-bold text-fg-subtle">
						<tr>
							<th scope="col" class="px-2 py-2">In Huxley</th>
							<th scope="col" class="px-2 py-2">In {data.target}</th>
							<th scope="col" class="w-40 px-2 py-2"><span class="sr-only">Change</span></th>
						</tr>
					</thead>
					<tbody>
						{#each locals[tab] as l (l.key)}
							{@const m = matchOf(tab, l.key)}
							<tr class="border-b border-border last:border-0">
								<td class="px-2 py-1.5 text-fg">{l.name}</td>
								<td class="px-2 py-1.5">
									{#if data.canConfigure}
										<form id="map-{tab}-{l.key}" method="POST" action="?/map" use:enhance={said()}>
											<input type="hidden" name="orgId" value={data.orgId} />
											<input type="hidden" name="kind" value={tab} />
											<input type="hidden" name="localKey" value={l.key} />
											{#if data.options[tab].length}
												<select
													name="remote"
													class={select}
													aria-label="{l.name} in {data.target}"
													value={m ? `${m.remoteId}|${m.remoteName ?? ''}` : ''}
												>
													<option value="">Not matched</option>
													{#each data.options[tab] as o (o.id)}
														<option value="{o.id}|{o.name}">{o.name}</option>
													{/each}
												</select>
											{:else}
												<input
													name="remote"
													class={select}
													aria-label="{l.name} in {data.target}"
													placeholder="ID in {data.target}"
													value={m?.remoteId ?? ''}
												/>
											{/if}
										</form>
									{:else}
										<span class={m ? 'text-fg' : 'text-fg-subtle'}>
											{m ? (m.remoteName ?? m.remoteId) : 'Not matched'}
										</span>
									{/if}
								</td>
								<td class="px-2 py-1.5">
									{#if data.canConfigure}
										<div class="flex gap-1">
											<Button type="submit" size="sm" form="map-{tab}-{l.key}">Save</Button>
											{#if m}
												<form method="POST" action="?/unmap" use:enhance={said()}>
													<input type="hidden" name="orgId" value={data.orgId} />
													<input type="hidden" name="id" value={m.id} />
													<Button
														type="submit"
														size="sm"
														variant="ghost"
														aria-label="Unmatch {l.name}"
													>
														Unmatch
													</Button>
												</form>
											{/if}
										</div>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
				{#if tab === 'tax_code' && data.canConfigure}
					<div class="mt-3 flex items-end gap-2">
						<label class="text-sm text-fg">
							Another tax code
							<input bind:value={taxCode} class="{select} mt-1 w-40" maxlength="50" />
						</label>
						<Button
							size="sm"
							disabled={!taxCode.trim()}
							onclick={() => {
								extraTax.push(taxCode.trim());
								taxCode = '';
							}}
						>
							Add
						</Button>
					</div>
				{/if}
			{/if}
		</Card>
	{/if}
</div>
