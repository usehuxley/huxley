<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { untrack } from 'svelte';
	import { Plus, Trash2 } from '@lucide/svelte';
	import { Button, Card, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	type Task = {
		startOffsetDays: number | null;
		durationDays: number | null;
		key: string;
		title: string;
		estimateHours: number | null;
		dependsOn: string[];
		clientVisible: boolean;
	};
	type Phase = { name: string; budgetHours: number | null; tasks: Task[] };
	type Mile = { name: string; offsetDays: number; clientVisible: boolean };
	const initial = untrack(() => data.template);
	let name = $state(initial?.name ?? '');
	let description = $state(initial?.description ?? '');
	let phases: Phase[] = $state(
		(initial?.structure.phases ?? [{ name: 'Phase 1', budgetHours: null, tasks: [] }]).map((p) => ({
			name: p.name,
			budgetHours: p.budgetHours ?? null,
			tasks: (p.tasks ?? []).map((t) => ({
				key: t.key,
				title: t.title,
				estimateHours: t.estimateHours ?? null,
				dependsOn: [...(t.dependsOn ?? [])],
				clientVisible: t.clientVisible ?? false,
				startOffsetDays: t.startOffsetDays ?? null,
				durationDays: t.durationDays ?? null
			}))
		}))
	);
	let milestones: Mile[] = $state(
		(initial?.structure.milestones ?? []).map((m) => ({
			name: m.name,
			offsetDays: m.offsetDays ?? 0,
			clientVisible: m.clientVisible ?? true
		}))
	);
	let counter = 0;
	const newKey = () => `t${Date.now().toString(36)}${counter++}`;
	let structure = $derived(
		JSON.stringify({
			phases: phases.map((p) => ({
				name: p.name,
				budgetHours:
					p.budgetHours === null || Number.isNaN(p.budgetHours) ? null : Number(p.budgetHours),
				tasks: (p.tasks ?? []).map((t) => ({
					key: t.key,
					title: t.title,
					estimateHours:
						t.estimateHours === null || Number.isNaN(t.estimateHours)
							? null
							: Number(t.estimateHours),
					dependsOn: t.dependsOn,
					clientVisible: t.clientVisible,
					startOffsetDays:
						t.startOffsetDays === null || Number.isNaN(t.startOffsetDays)
							? undefined
							: Number(t.startOffsetDays),
					durationDays:
						t.durationDays === null || Number.isNaN(t.durationDays)
							? undefined
							: Number(t.durationDays)
				}))
			})),
			milestones: milestones.map((m) => ({
				name: m.name,
				offsetDays: Number(m.offsetDays) || 0,
				clientVisible: m.clientVisible
			}))
		})
	);
	let allTasks = $derived(phases.flatMap((p) => p.tasks));
</script>

<svelte:head>
	<title>{initial ? initial.name : 'New template'} · Project templates · Huxley</title>
</svelte:head>

<form method="POST" action="?/save" class="flex flex-col gap-4" use:enhance>
	<input type="hidden" name="structure" value={structure} />
	{#if initial}
		<input type="hidden" name="version" value={initial.version} />
		<input type="hidden" name="orgId" value={initial.orgId} />
	{/if}
	<Card title={initial ? 'Edit template' : 'New template'}>
		<div class="grid gap-3 sm:grid-cols-2">
			<Field label="Template name" for="tp-name">
				<Input id="tp-name" name="name" bind:value={name} required />
			</Field>
			<Field label="Description" for="tp-desc">
				<Input id="tp-desc" name="description" bind:value={description} />
			</Field>
		</div>
	</Card>
	{#each phases as phase, i (i)}
		<Card>
			<div class="flex flex-col gap-2" role="group" aria-label="Phase {i + 1}">
				<div class="grid gap-2 sm:grid-cols-[1fr_8rem_auto] sm:items-end">
					<Field label="Phase name" for="ph-{i}">
						<Input id="ph-{i}" bind:value={phase.name} required />
					</Field>
					<Field label="Budget hours" for="phb-{i}">
						<Input id="phb-{i}" type="number" min="0" step="0.5" bind:value={phase.budgetHours} />
					</Field>
					<Button
						type="button"
						variant="ghost"
						aria-label="Remove phase {i + 1}"
						onclick={() => phases.splice(i, 1)}
					>
						<Trash2 size={16} />
					</Button>
				</div>
				{#each phase.tasks as task, j (task.key)}
					<div class="grid gap-2 sm:grid-cols-[1fr_5rem_6rem_6rem_10rem_auto_auto] sm:items-end">
						<Field label="Task" for="tk-{task.key}">
							<Input id="tk-{task.key}" bind:value={task.title} required />
						</Field>
						<Field label="Hours" for="tke-{task.key}">
							<Input
								id="tke-{task.key}"
								type="number"
								min="0"
								step="0.5"
								bind:value={task.estimateHours}
							/>
						</Field>
						<Field label="Starts on day" for="tks-{task.key}">
							<Input id="tks-{task.key}" type="number" min="0" bind:value={task.startOffsetDays} />
						</Field>
						<Field label="Takes days" for="tkn-{task.key}">
							<Input id="tkn-{task.key}" type="number" min="0" bind:value={task.durationDays} />
						</Field>
						<Field label="After" for="tkd-{task.key}">
							<select
								id="tkd-{task.key}"
								class="h-9 w-full rounded-control border border-border-strong bg-surface px-2 text-sm text-fg"
								value={task.dependsOn[0] ?? ''}
								onchange={(e) => {
									const v = e.currentTarget.value;
									task.dependsOn = v ? [v] : [];
								}}
							>
								<option value="">Any time</option>
								{#each allTasks.filter((t) => t.key !== task.key) as other (other.key)}
									<option value={other.key}>{other.title || 'Untitled'}</option>
								{/each}
							</select>
						</Field>
						<label class="flex h-9 items-center gap-1 text-sm text-fg">
							<input type="checkbox" bind:checked={task.clientVisible} /> Client sees it
						</label>
						<Button
							type="button"
							variant="ghost"
							aria-label="Remove task"
							onclick={() => phase.tasks.splice(j, 1)}
						>
							<Trash2 size={16} />
						</Button>
					</div>
				{/each}
				<div>
					<Button
						type="button"
						variant="ghost"
						onclick={() =>
							phase.tasks.push({
								key: newKey(),
								title: '',
								estimateHours: null,
								dependsOn: [],
								clientVisible: false,
								startOffsetDays: null,
								durationDays: null
							})}
					>
						<Plus size={16} /> Add a task
					</Button>
				</div>
			</div>
		</Card>
	{/each}
	<div>
		<Button
			type="button"
			onclick={() =>
				phases.push({ name: `Phase ${phases.length + 1}`, budgetHours: null, tasks: [] })}
		>
			<Plus size={16} /> Add a phase
		</Button>
	</div>
	<Card title="Milestones">
		<div class="flex flex-col gap-2">
			{#each milestones as m, i (i)}
				<div class="grid gap-2 sm:grid-cols-[1fr_8rem_auto_auto] sm:items-end">
					<Field label="Milestone" for="ms-{i}">
						<Input id="ms-{i}" bind:value={m.name} required />
					</Field>
					<Field label="Days after start" for="msd-{i}">
						<Input id="msd-{i}" type="number" min="0" bind:value={m.offsetDays} />
					</Field>
					<label class="flex h-9 items-center gap-1 text-sm text-fg">
						<input type="checkbox" bind:checked={m.clientVisible} /> Client sees it
					</label>
					<Button
						type="button"
						variant="ghost"
						aria-label="Remove milestone"
						onclick={() => milestones.splice(i, 1)}
					>
						<Trash2 size={16} />
					</Button>
				</div>
			{/each}
			<div>
				<Button
					type="button"
					variant="ghost"
					onclick={() => milestones.push({ name: '', offsetDays: 0, clientVisible: true })}
				>
					<Plus size={16} /> Add a milestone
				</Button>
			</div>
		</div>
	</Card>
	{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
	<div class="flex flex-wrap gap-2">
		<Button type="submit" variant="primary">Save template</Button>
		<Button href={resolve('/(app)/settings/project-templates')} variant="ghost">Cancel</Button>
	</div>
</form>
{#if initial}
	<Card
		title="Workflows"
		description="Workflows that run for projects made from this template: on project events, filtered to it."
		class="mt-4"
	>
		<div class="flex flex-col gap-2 text-sm">
			<ul class="flex flex-col gap-1" aria-label="Template workflows">
				{#each data.workflows as w (w.id)}
					<li class="flex items-center justify-between gap-2">
						<a
							href={resolve('/(app)/workflows/[id]', { id: w.id })}
							class="text-accent hover:underline"
						>
							{w.name}
						</a>
						<span class="text-fg-muted">on {w.event} · {w.status}</span>
					</li>
				{:else}
					<li class="text-fg-muted">None yet.</li>
				{/each}
			</ul>
			<form method="POST" action="?/workflow" use:enhance>
				<Button type="submit" variant="ghost">
					<Plus size={16} /> New workflow for this template
				</Button>
			</form>
		</div>
	</Card>
	<form method="POST" action="?/remove" class="mt-4" use:enhance>
		<Button type="submit" variant="ghost"><Trash2 size={16} /> Delete template</Button>
	</form>
{/if}
