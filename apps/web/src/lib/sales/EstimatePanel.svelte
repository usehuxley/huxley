<!-- A scope's estimate: the Work Breakdown Structure (hours by role,
     low / likely / high), the Bill of Materials, contingency by risk, and
     the checks before building the quote. -->
<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Hammer, Plus, Sparkles, Trash2 } from '@lucide/svelte';
	import { Badge, Button, Card, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import { money } from '#lib/sales/labels.ts';

	type Estimate = components['schemas']['ScopeEstimate'];
	type Option = { value: string; label: string };
	let {
		estimate,
		roles,
		products,
		canWrite
	}: { estimate: Estimate; roles: Option[]; products: Option[]; canWrite: boolean } = $props();

	type TaskRow = {
		key: number;
		phase: string;
		name: string;
		roleProductId: string;
		hoursLow: string;
		hoursLikely: string;
		hoursHigh: string;
		deliverable: string;
		milestone: boolean;
		drafted: boolean;
	};
	type ItemRow = {
		key: number;
		productId: string;
		name: string;
		quantity: string;
		basis: string;
		alternate: boolean;
		drafted: boolean;
	};
	let seq = 0;
	let tasks = $state<TaskRow[]>([]);
	let items = $state<ItemRow[]>([]);
	let tasksDirty = $state(false);
	let itemsDirty = $state(false);
	$effect(() => {
		tasks = estimate.tasks.map((t) => ({
			key: ++seq,
			phase: t.phase,
			name: t.name,
			roleProductId: t.roleProductId ?? '',
			hoursLow: t.hoursLow,
			hoursLikely: t.hoursLikely,
			hoursHigh: t.hoursHigh,
			deliverable: t.deliverable ?? '',
			milestone: t.milestone,
			drafted: t.drafted
		}));
		items = estimate.items.map((i) => ({
			key: ++seq,
			productId: i.productId ?? '',
			name: i.name,
			quantity: i.quantity,
			basis: i.basis ?? '',
			alternate: i.alternate,
			drafted: i.drafted
		}));
		tasksDirty = false;
		itemsDirty = false;
	});

	let tasksDoc = $derived(
		JSON.stringify(
			tasks.map((t) => ({
				phase: t.phase,
				name: t.name,
				roleProductId: t.roleProductId || null,
				hoursLow: t.hoursLow || '0',
				hoursLikely: t.hoursLikely || '0',
				hoursHigh: t.hoursHigh || '0',
				deliverable: t.deliverable || null,
				milestone: t.milestone,
				drafted: t.drafted
			}))
		)
	);
	let itemsDoc = $derived(
		JSON.stringify(
			items.map((i) => ({
				productId: i.productId || null,
				name: i.name || products.find((p) => p.value === i.productId)?.label || 'Item',
				quantity: i.quantity || '1',
				basis: i.basis || null,
				alternate: i.alternate,
				drafted: i.drafted
			}))
		)
	);

	const done =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			const said = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (said as { message?: string } | undefined)?.message;
			if (result.type === 'success') {
				toast.push({ title: message ?? 'Saved.', tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				toast.push({ title: message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
	const cell = 'h-8 w-full rounded-control border border-border bg-surface px-2 text-sm text-fg';
	const num = `${cell} text-right tabular-nums`;
	let waiting = $derived(estimate.state === 'requested');
</script>

<Card title="Estimate">
	{#snippet actions()}
		{#if canWrite}
			<div class="flex flex-wrap gap-2">
				<form method="POST" action="?/estimate" use:enhance={done()}>
					{#if waiting}
						<input type="hidden" name="cancel" value="yes" />
						<Button type="submit" size="sm" variant="ghost">Stop waiting</Button>
					{:else}
						<Button type="submit" size="sm"><Sparkles size={14} /> Estimate with AI</Button>
					{/if}
				</form>
				<form method="POST" action="?/assemble" use:enhance={done()}>
					<Button type="submit" size="sm" variant="primary" disabled={tasksDirty || itemsDirty}>
						<Hammer size={14} /> Build the quote
					</Button>
				</form>
			</div>
		{/if}
	{/snippet}
	<div class="flex flex-col gap-5">
		{#if waiting}
			<p class="text-sm text-fg-subtle">Estimating with AI…</p>
		{:else if estimate.state === 'failed' && estimate.error}
			<p class="rounded-control bg-danger-soft px-3 py-2 text-sm text-fg">{estimate.error}</p>
		{/if}

		<dl class="grid gap-3 sm:grid-cols-4" aria-label="Estimate totals">
			<div>
				<dt class="text-xs font-bold text-fg-subtle">Hours (with contingency)</dt>
				<dd class="font-mono text-lg font-semibold tabular-nums">{Number(estimate.hours)}</dd>
			</div>
			<div>
				<dt class="text-xs font-bold text-fg-subtle">Labour</dt>
				<dd class="font-mono text-lg font-semibold tabular-nums">{money(estimate.labour)}</dd>
			</div>
			<div>
				<dt class="text-xs font-bold text-fg-subtle">Materials</dt>
				<dd class="font-mono text-lg font-semibold tabular-nums">{money(estimate.materials)}</dd>
			</div>
			<div>
				<dt class="text-xs font-bold text-fg-subtle">Risk · contingency</dt>
				<dd>
					{#if canWrite}
						<form
							method="POST"
							action="?/risk"
							use:enhance={done()}
							class="flex items-center gap-2"
						>
							<label class="sr-only" for="risk">Risk</label>
							<select
								id="risk"
								name="risk"
								class="h-8 rounded-control border border-border-strong bg-surface px-2 text-sm text-fg"
								value={estimate.risk}
								onchange={(e) => e.currentTarget.form?.requestSubmit()}
							>
								<option value="low">Low</option>
								<option value="medium">Medium</option>
								<option value="high">High</option>
							</select>
							<span class="text-sm text-fg-muted">{Number(estimate.contingencyPct)}%</span>
						</form>
					{:else}
						<span class="text-sm">{estimate.risk} · {Number(estimate.contingencyPct)}%</span>
					{/if}
				</dd>
			</div>
		</dl>

		{#if estimate.warnings.length}
			<ul
				class="flex flex-col gap-1 rounded-control bg-warning-soft px-3 py-2 text-sm text-fg"
				aria-label="Before quoting"
			>
				{#each estimate.warnings as w, i (i)}<li>{w}</li>{/each}
			</ul>
		{/if}

		<section aria-label="Work breakdown">
			<h3 class="mb-2 text-sm font-semibold text-fg">Work breakdown</h3>
			<div class="overflow-x-auto">
				<table class="w-full min-w-[860px] text-left text-sm">
					<thead class="text-xs font-bold text-fg-subtle">
						<tr>
							<th scope="col" class="w-32 px-1 py-1">Phase</th>
							<th scope="col" class="px-1 py-1">Task</th>
							<th scope="col" class="w-40 px-1 py-1">Role</th>
							<th scope="col" class="w-16 px-1 py-1 text-right">Low</th>
							<th scope="col" class="w-16 px-1 py-1 text-right">Likely</th>
							<th scope="col" class="w-16 px-1 py-1 text-right">High</th>
							<th scope="col" class="w-44 px-1 py-1">Deliverable</th>
							<th scope="col" class="w-8"><span class="sr-only">Remove</span></th>
						</tr>
					</thead>
					<tbody>
						{#each tasks as t, i (t.key)}
							<tr>
								<td class="px-1 py-0.5">
									<input
										class={cell}
										aria-label="Phase"
										bind:value={t.phase}
										oninput={() => ((tasksDirty = true), (t.drafted = false))}
										disabled={!canWrite}
									/>
								</td>
								<td class="px-1 py-0.5">
									<input
										class={cell}
										aria-label="Task"
										bind:value={t.name}
										oninput={() => ((tasksDirty = true), (t.drafted = false))}
										disabled={!canWrite}
									/>
								</td>
								<td class="px-1 py-0.5">
									<select
										class={cell}
										aria-label="Role"
										bind:value={t.roleProductId}
										onchange={() => ((tasksDirty = true), (t.drafted = false))}
										disabled={!canWrite}
									>
										<option value="">No role</option>
										{#each roles as r (r.value)}<option value={r.value}>{r.label}</option>{/each}
									</select>
								</td>
								<td class="px-1 py-0.5">
									<input
										class={num}
										aria-label="Low hours"
										inputmode="decimal"
										bind:value={t.hoursLow}
										oninput={() => ((tasksDirty = true), (t.drafted = false))}
										disabled={!canWrite}
									/>
								</td>
								<td class="px-1 py-0.5">
									<input
										class={num}
										aria-label="Likely hours"
										inputmode="decimal"
										bind:value={t.hoursLikely}
										oninput={() => ((tasksDirty = true), (t.drafted = false))}
										disabled={!canWrite}
									/>
								</td>
								<td class="px-1 py-0.5">
									<input
										class={num}
										aria-label="High hours"
										inputmode="decimal"
										bind:value={t.hoursHigh}
										oninput={() => ((tasksDirty = true), (t.drafted = false))}
										disabled={!canWrite}
									/>
								</td>
								<td class="px-1 py-0.5">
									<input
										class={cell}
										aria-label="Deliverable"
										bind:value={t.deliverable}
										oninput={() => ((tasksDirty = true), (t.drafted = false))}
										disabled={!canWrite}
									/>
								</td>
								<td class="px-1 py-0.5">
									{#if canWrite}
										<button
											type="button"
											class="grid size-8 place-items-center text-fg-subtle hover:text-danger"
											aria-label="Remove {t.name || 'task'}"
											onclick={() => (tasks.splice(i, 1), (tasksDirty = true))}
										>
											<Trash2 size={14} />
										</button>
									{/if}
								</td>
							</tr>
						{:else}
							<tr>
								<td colspan="8" class="px-1 py-4 text-center text-sm text-fg-subtle">
									No tasks yet.
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
			{#if canWrite}
				<form method="POST" action="?/tasks" class="mt-2 flex gap-2" use:enhance={done()}>
					<input type="hidden" name="doc" value={tasksDoc} />
					<Button
						size="sm"
						onclick={() => (
							tasks.push({
								key: ++seq,
								phase: tasks.at(-1)?.phase ?? 'Delivery',
								name: '',
								roleProductId: roles[0]?.value ?? '',
								hoursLow: '1',
								hoursLikely: '2',
								hoursHigh: '4',
								deliverable: '',
								milestone: false,
								drafted: false
							}),
							(tasksDirty = true)
						)}
					>
						<Plus size={14} /> Task
					</Button>
					<Button type="submit" size="sm" variant={tasksDirty ? 'primary' : 'secondary'}>
						Save the work breakdown
					</Button>
				</form>
			{/if}
			{#if estimate.roles.length}
				<ul class="mt-2 flex flex-wrap gap-2 text-xs" aria-label="Hours by role">
					{#each estimate.roles as r, i (i)}
						<li>
							<Badge>{r.role}: {Number(r.hours)} h{r.cost ? ` · ${money(r.cost)}` : ''}</Badge>
						</li>
					{/each}
				</ul>
			{/if}
		</section>

		<section aria-label="Bill of materials">
			<h3 class="mb-2 text-sm font-semibold text-fg">Bill of materials</h3>
			<div class="overflow-x-auto">
				<table class="w-full min-w-[720px] text-left text-sm">
					<thead class="text-xs font-bold text-fg-subtle">
						<tr>
							<th scope="col" class="w-64 px-1 py-1">Product</th>
							<th scope="col" class="w-20 px-1 py-1 text-right">Quantity</th>
							<th scope="col" class="px-1 py-1">Basis</th>
							<th scope="col" class="w-20 px-1 py-1 text-center">Alternate</th>
							<th scope="col" class="w-8"><span class="sr-only">Remove</span></th>
						</tr>
					</thead>
					<tbody>
						{#each items as x, i (x.key)}
							<tr>
								<td class="px-1 py-0.5">
									<select
										class={cell}
										aria-label="Product"
										bind:value={x.productId}
										onchange={() => ((itemsDirty = true), (x.drafted = false), (x.name = ''))}
										disabled={!canWrite}
									>
										<option value="">{x.name || 'Not in the catalog'}</option>
										{#each products as p (p.value)}<option value={p.value}>{p.label}</option>{/each}
									</select>
								</td>
								<td class="px-1 py-0.5">
									<input
										class={num}
										aria-label="Quantity"
										inputmode="decimal"
										bind:value={x.quantity}
										oninput={() => ((itemsDirty = true), (x.drafted = false))}
										disabled={!canWrite}
									/>
								</td>
								<td class="px-1 py-0.5">
									<input
										class={cell}
										aria-label="Basis"
										bind:value={x.basis}
										oninput={() => ((itemsDirty = true), (x.drafted = false))}
										disabled={!canWrite}
									/>
								</td>
								<td class="px-1 py-0.5 text-center">
									<input
										type="checkbox"
										aria-label="An alternate"
										bind:checked={x.alternate}
										onchange={() => ((itemsDirty = true), (x.drafted = false))}
										disabled={!canWrite}
									/>
								</td>
								<td class="px-1 py-0.5">
									{#if canWrite}
										<button
											type="button"
											class="grid size-8 place-items-center text-fg-subtle hover:text-danger"
											aria-label="Remove line"
											onclick={() => (items.splice(i, 1), (itemsDirty = true))}
										>
											<Trash2 size={14} />
										</button>
									{/if}
								</td>
							</tr>
						{:else}
							<tr>
								<td colspan="5" class="px-1 py-4 text-center text-sm text-fg-subtle">
									Nothing to buy yet.
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
			{#if canWrite}
				<form method="POST" action="?/items" class="mt-2 flex gap-2" use:enhance={done()}>
					<input type="hidden" name="doc" value={itemsDoc} />
					<Button
						size="sm"
						onclick={() => (
							items.push({
								key: ++seq,
								productId: products[0]?.value ?? '',
								name: '',
								quantity: '1',
								basis: '',
								alternate: false,
								drafted: false
							}),
							(itemsDirty = true)
						)}
					>
						<Plus size={14} /> Line
					</Button>
					<Button type="submit" size="sm" variant={itemsDirty ? 'primary' : 'secondary'}>
						Save the materials
					</Button>
				</form>
			{/if}
		</section>
	</div>
</Card>
