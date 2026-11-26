<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, Field, Input, Select } from '@usehuxley/ui';
	import { BILLING, HEALTH, STATUSES, label } from '#lib/projects/labels.ts';
	import Board from '#lib/projects/Board.svelte';
	import ScopeChanges from '#lib/projects/ScopeChanges.svelte';
	import Timeline from '#lib/projects/Timeline.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let p = $derived(data.project);
	let groups = $derived([
		...p.phases.map((ph) => ({
			id: ph.id,
			name: ph.name,
			budget: ph.budgetHours,
			used: ph.usedHours
		})),
		{ id: null, name: 'Other tasks', budget: null, used: null }
	]);
	const tasksIn = (phase: string | null) => p.tasks.filter((t) => (t.phaseId ?? null) === phase);
	const done = (c: string) => c === 'resolved' || c === 'closed';
	let burn = $derived(Math.min(100, p.burnPercent ?? 0));
</script>

<svelte:head><title>{p.name} · Huxley</title></svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-4">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<a href={resolve('/(app)/projects')} class="text-sm text-accent hover:underline">Projects</a>
			<h1 class="mt-1 flex flex-wrap items-center gap-2 text-2xl font-semibold text-fg">
				{p.name}
				<Badge tone={HEALTH[p.health.level]?.[1] ?? 'neutral'}>
					{HEALTH[p.health.level]?.[0] ?? p.health.level}
				</Badge>
			</h1>
			<p class="text-fg-muted">
				{p.orgName} · {p.startOn}{p.endOn ? ` to ${p.endOn}` : ''} · {label(
					BILLING,
					p.billingMethod
				)}{p.managerName ? ` · managed by ${p.managerName}` : ''}{p.agreementName
					? ` · billed to ${p.agreementName}`
					: ''}
			</p>
		</div>
		{#if data.canManage}
			<form method="POST" action="?/status" class="flex items-end gap-2" use:enhance>
				<input type="hidden" name="version" value={p.version} />
				<div class="w-40">
					<Select
						aria-label="Status"
						name="status"
						value={p.status}
						options={STATUSES.map((s) => ({ value: s.value, label: s.label }))}
					/>
				</div>
				<div class="w-48">
					<Select
						aria-label="Agreement"
						name="agreementId"
						value={p.agreementId ?? ''}
						options={[{ value: '', label: 'No agreement' }, ...data.agreements]}
					/>
				</div>
				<Button type="submit">Save</Button>
			</form>
		{/if}
	</div>
	{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}

	<div class="grid gap-4 md:grid-cols-3">
		<Card title="Budget">
			<p class="text-2xl font-semibold text-fg tabular-nums" aria-label="Budget used">
				{p.usedHours}{p.budgetHours !== null && p.budgetHours !== undefined
					? ` of ${p.budgetHours}`
					: ''} hours
			</p>
			{#if p.burnPercent !== null && p.burnPercent !== undefined}
				<div class="mt-2 h-2 rounded bg-surface-sunken" aria-hidden="true">
					<div
						class="h-2 rounded {p.burnPercent > 100
							? 'bg-danger'
							: p.burnPercent >= 75
								? 'bg-warning'
								: 'bg-accent'}"
						style="width: {burn}%"
					></div>
				</div>
				<p class="mt-1 text-sm text-fg-muted">
					{p.burnPercent}% used · alerts at {p.thresholds.join(', ')}%
				</p>
			{/if}
		</Card>
		<Card title="Progress">
			<p class="text-2xl font-semibold text-fg tabular-nums">
				{p.progress.done} of {p.progress.total} tasks done
			</p>
			{#if p.health.reasons.length > 0}
				<ul class="mt-2 text-sm text-fg-muted">
					{#each p.health.reasons as r (r)}<li>{r}</li>{/each}
				</ul>
			{/if}
		</Card>
		<Card title="Milestones">
			<ul class="flex flex-col gap-2 text-sm" aria-label="Milestones">
				{#each p.milestones as m (m.id)}
					<li class="flex items-center justify-between gap-2">
						<span class="text-fg">
							{m.name}
							<span class="text-fg-muted">{m.dueOn ? `· due ${m.dueOn}` : ''}</span>
						</span>
						{#if m.reachedAt}
							<span class="flex items-center gap-1">
								<Badge tone="success">Reached</Badge>
								{#if data.canWrite}
									<form method="POST" action="?/milestone" use:enhance>
										<input type="hidden" name="milestone" value={m.id} />
										<input type="hidden" name="reached" value="false" />
										<Button type="submit" variant="ghost" size="sm">Undo</Button>
									</form>
								{/if}
							</span>
						{:else if data.canWrite}
							<form method="POST" action="?/milestone" use:enhance>
								<input type="hidden" name="milestone" value={m.id} />
								<input type="hidden" name="reached" value="true" />
								<Button type="submit" size="sm" aria-label="Mark {m.name} reached">Reached</Button>
							</form>
						{/if}
					</li>
				{:else}
					<li class="text-fg-muted">None.</li>
				{/each}
			</ul>
		</Card>
	</div>

	<nav class="flex gap-1 border-b border-border" aria-label="Views">
		{#each [['list', 'List'], ['board', 'Board'], ['timeline', 'Timeline']] as [key, name] (key)}
			<a
				href="{resolve('/(app)/projects/[id]', { id: p.id })}?view={key}"
				aria-current={data.view === key ? 'page' : undefined}
				class="-mb-px border-b-2 px-3 py-1.5 text-sm {data.view === key
					? 'border-accent font-medium text-fg'
					: 'border-transparent text-fg-muted hover:text-fg'}"
			>
				{name}
			</a>
		{/each}
	</nav>
	{#if data.view === 'board'}
		<Board tasks={p.tasks} />
	{:else if data.view === 'timeline'}
		<Card><Timeline project={p} /></Card>
	{/if}

	<Card title="Tasks" class={data.view === 'list' ? '' : 'hidden'}>
		<div class="flex flex-col gap-4">
			{#each groups as g (g.id ?? 'other')}
				{#if g.id !== null || tasksIn(null).length > 0}
					<section>
						<h2 class="mb-1 flex items-baseline gap-2 text-sm font-semibold text-fg">
							{g.name}
							{#if g.budget !== null && g.budget !== undefined}
								<span class="font-normal text-fg-muted">{g.used} of {g.budget} hours</span>
							{/if}
						</h2>
						<ul class="flex flex-col divide-y divide-border" aria-label="{g.name} tasks">
							{#each tasksIn(g.id) as t (t.ticketId)}
								<li class="flex flex-wrap items-center justify-between gap-2 py-1.5 text-sm">
									<a
										href={resolve('/(app)/tickets/[id]', { id: t.ticketId })}
										class="text-accent hover:underline {done(t.statusCategory)
											? 'line-through'
											: ''}"
									>
										{t.reference} · {t.title}
									</a>
									<span class="flex flex-wrap items-center gap-2 text-fg-muted">
										{#if t.waiting}<Badge tone="warning">Waiting on another task</Badge>{/if}
										<span>{t.assigneeName ?? 'Unassigned'}</span>
										{#if t.startOn || t.dueOn}
											<span>{t.startOn ?? '…'} → {t.dueOn ?? '…'}</span>
										{/if}
										<span class="tabular-nums">
											{t.usedHours}{t.estimateHours ? `/${t.estimateHours}` : ''} h
										</span>
										<Badge>{t.statusName}</Badge>
									</span>
									{#if data.suggestions[t.ticketId]}
										<div
											class="flex w-full flex-wrap items-center gap-2 text-xs"
											aria-label="Who could take {t.reference}"
										>
											<span class="text-fg-muted">Could take it:</span>
											{#each data.suggestions[t.ticketId] as s (s.userId)}
												<form method="POST" action="?/assign" use:enhance>
													<input type="hidden" name="ticket" value={t.ticketId} />
													<input type="hidden" name="userId" value={s.userId} />
													<Button
														type="submit"
														size="sm"
														variant="ghost"
														aria-label="Assign {t.reference} to {s.name}"
													>
														{s.name} · {s.skillMatch}% skills{s.freeHours !== null
															? ` · ${s.freeHours} h free`
															: ''}
													</Button>
												</form>
											{/each}
										</div>
									{/if}
									{#if data.canWrite}
										<details class="w-full">
											<summary class="cursor-pointer text-xs text-fg-muted">
												Edit {t.reference}
											</summary>
											<form
												method="POST"
												action="?/editTask"
												class="mt-2 grid gap-2 sm:grid-cols-[10rem_6rem_10rem_10rem_auto_auto] sm:items-end"
												use:enhance
											>
												<input type="hidden" name="ticket" value={t.ticketId} />
												<Field label="Phase" for="et-phase-{t.ticketId}">
													<Select
														id="et-phase-{t.ticketId}"
														name="phaseId"
														value={t.phaseId ?? ''}
														options={[
															{ value: '', label: 'None' },
															...p.phases.map((ph) => ({ value: ph.id, label: ph.name }))
														]}
													/>
												</Field>
												<Field label="Hours" for="et-hours-{t.ticketId}">
													<Input
														id="et-hours-{t.ticketId}"
														name="estimateHours"
														type="number"
														min="0"
														step="0.5"
														value={t.estimateHours ?? ''}
													/>
												</Field>
												<Field label="Starts" for="et-start-{t.ticketId}">
													<Input
														id="et-start-{t.ticketId}"
														name="startOn"
														type="date"
														value={t.startOn ?? ''}
													/>
												</Field>
												<Field label="Due" for="et-due-{t.ticketId}">
													<Input
														id="et-due-{t.ticketId}"
														name="dueOn"
														type="date"
														value={t.dueOn ?? ''}
													/>
												</Field>
												<label class="flex h-9 items-center gap-1 text-sm text-fg">
													<input type="checkbox" name="clientVisible" checked={t.clientVisible} /> Client
													sees it
												</label>
												<Button type="submit" size="sm">Save task</Button>
											</form>
										</details>
									{/if}
								</li>
							{:else}
								<li class="py-1.5 text-sm text-fg-muted">No tasks.</li>
							{/each}
						</ul>
					</section>
				{/if}
			{/each}
			{#if data.canWrite}
				<form
					method="POST"
					action="?/task"
					class="grid gap-2 border-t border-border pt-3 sm:grid-cols-[1fr_10rem_12rem_6rem_auto] sm:items-end"
					use:enhance
				>
					<Field label="New task" for="nt-title">
						<Input id="nt-title" name="title" required />
					</Field>
					<Field label="Phase" for="nt-phase">
						<Select
							id="nt-phase"
							name="phaseId"
							options={[
								{ value: '', label: 'None' },
								...p.phases.map((ph) => ({ value: ph.id, label: ph.name }))
							]}
						/>
					</Field>
					<Field label="Assignee" for="nt-assignee">
						<Select
							id="nt-assignee"
							name="assigneeUserId"
							options={[{ value: '', label: 'Unassigned' }, ...data.people]}
						/>
					</Field>
					<Field label="Hours" for="nt-hours">
						<Input id="nt-hours" name="estimateHours" type="number" min="0" step="0.5" />
					</Field>
					<Button type="submit">Add task</Button>
					<Field label="Starts" for="nt-start">
						<Input id="nt-start" name="startOn" type="date" />
					</Field>
					<Field label="Due" for="nt-due"><Input id="nt-due" name="dueOn" type="date" /></Field>
				</form>
			{/if}
		</div>
	</Card>

	<ScopeChanges changes={p.scopeChanges} canManage={data.canManage} />
</div>
