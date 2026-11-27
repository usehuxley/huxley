<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { ArrowDown, ArrowUp, Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	type Stage = (typeof data.stages)[number];
	let open = $state(false);
	let editing: Stage | null = $state(null);
	let kind = $state('open');

	const done =
		(message: string, after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
	const KINDS = [
		{ value: 'open', label: 'Open (worked)' },
		{ value: 'won', label: 'Won' },
		{ value: 'lost', label: 'Lost' }
	];
</script>

<svelte:head>
	<title>Sales pipeline · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Stages"
		description="What an opportunity moves through, in order, with the chance of winning at each. Won and lost stages close it."
	>
		{#snippet actions()}
			<Button
				onclick={() => {
					editing = null;
					kind = 'open';
					open = true;
				}}
			>
				<Plus size={16} /> New stage
			</Button>
		{/snippet}
		<ol class="flex flex-col divide-y divide-border" aria-label="Stages">
			{#each data.stages as s, i (s.id)}
				<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
					<div class="flex min-w-0 items-center gap-2">
						<span class="w-6 font-mono text-xs text-fg-subtle">{i + 1}</span>
						<span class="text-sm font-semibold text-fg">{s.name}</span>
						<Badge tone={s.kind === 'won' ? 'success' : s.kind === 'lost' ? 'danger' : 'info'}>
							{s.kind === 'open' ? `${s.probability}%` : s.kind === 'won' ? 'Won' : 'Lost'}
						</Badge>
					</div>
					<div class="flex shrink-0 gap-1">
						<form method="POST" action="?/reorder" use:enhance={done('Moved.')} class="flex gap-1">
							{#each data.stages as x (x.id)}<input type="hidden" name="ids" value={x.id} />{/each}
							<input type="hidden" name="id" value={s.id} />
							<Button
								type="submit"
								name="dir"
								value="up"
								size="sm"
								variant="ghost"
								disabled={i === 0}
								aria-label="Move {s.name} up"
							>
								<ArrowUp size={14} />
							</Button>
							<Button
								type="submit"
								name="dir"
								value="down"
								size="sm"
								variant="ghost"
								disabled={i === data.stages.length - 1}
								aria-label="Move {s.name} down"
							>
								<ArrowDown size={14} />
							</Button>
						</form>
						<Button
							size="sm"
							onclick={() => {
								editing = s;
								kind = s.kind;
								open = true;
							}}
						>
							Edit
						</Button>
						<form method="POST" action="?/archiveStage" use:enhance={done('Removed.')}>
							<input type="hidden" name="id" value={s.id} />
							<Button type="submit" size="sm" variant="ghost" aria-label="Remove {s.name}">
								Remove
							</Button>
						</form>
					</div>
				</li>
			{/each}
		</ol>
	</Card>

	<div class="grid gap-6 md:grid-cols-2">
		{#each ['won', 'lost'] as k (k)}
			<Card title={k === 'won' ? 'Why deals are won' : 'Why deals are lost'}>
				<ul
					class="flex flex-col divide-y divide-border"
					aria-label={k === 'won' ? 'Win reasons' : 'Loss reasons'}
				>
					{#each data.reasons.filter((r) => r.kind === k) as r (r.id)}
						<li class="flex items-center justify-between gap-3 py-1.5 first:pt-0">
							<span class="text-sm text-fg">{r.name}</span>
							<form method="POST" action="?/archiveReason" use:enhance={done('Removed.')}>
								<input type="hidden" name="id" value={r.id} />
								<Button type="submit" size="sm" variant="ghost" aria-label="Remove {r.name}">
									Remove
								</Button>
							</form>
						</li>
					{/each}
				</ul>
				<form
					method="POST"
					action="?/addReason"
					class="mt-3 flex gap-2"
					use:enhance={done('Added.')}
				>
					<input type="hidden" name="kind" value={k} />
					<label for="reason-{k}" class="sr-only">New reason</label>
					<Input
						id="reason-{k}"
						name="name"
						required
						maxlength={100}
						placeholder="A new reason"
						class="flex-1"
					/>
					<Button type="submit">Add</Button>
				</form>
			</Card>
		{/each}
	</div>
</div>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New stage'}>
	<form
		id="stage-form"
		method="POST"
		action="?/saveStage"
		class="flex flex-col gap-4"
		use:enhance={done('Saved.', () => (open = false))}
	>
		{#if editing}<input type="hidden" name="id" value={editing.id} />{/if}
		<Field label="Name" for="stage-name">
			<Input id="stage-name" name="name" required maxlength={60} value={editing?.name ?? ''} />
		</Field>
		<Field label="Kind" for="stage-kind">
			<Select id="stage-kind" name="kind" bind:value={kind} options={KINDS} />
		</Field>
		{#if kind === 'open'}
			<Field label="Chance of winning %" for="stage-probability">
				<Input
					id="stage-probability"
					name="probability"
					type="number"
					min={0}
					max={100}
					required
					value={editing?.probability ?? 50}
				/>
			</Field>
		{/if}
	</form>
	{#snippet footer()}
		<Button onclick={() => (open = false)}>Cancel</Button>
		<Button type="submit" form="stage-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>
