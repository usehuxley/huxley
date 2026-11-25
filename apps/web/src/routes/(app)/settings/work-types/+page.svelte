<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	type WorkType = (typeof data.workTypes)[number];
	let open = $state(false);
	let editing: WorkType | null = $state(null);
	let errors: Record<string, string> = $derived(form?.errors ?? {});

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
</script>

<svelte:head>
	<title>Work types · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Work types"
		description="What kind of work time is logged as: on tickets (remote support, onsite) or not (admin, training)."
	>
		{#snippet actions()}
			<Button
				onclick={() => {
					editing = null;
					open = true;
				}}
			>
				<Plus size={16} /> New work type
			</Button>
		{/snippet}
		<ul class="flex flex-col divide-y divide-border" aria-label="Work types">
			{#each data.workTypes as w (w.id)}
				<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
					<div class="flex min-w-0 flex-wrap items-center gap-2">
						<span class="text-sm text-fg">{w.name}</span>
						<Badge tone={w.billable ? 'success' : 'neutral'}>
							{w.billable ? 'Billable' : 'Not billable'}
						</Badge>
						{#if !w.onTickets}<Badge>Not on tickets</Badge>{/if}
					</div>
					<div class="flex shrink-0 gap-2">
						<Button
							size="sm"
							onclick={() => {
								editing = w;
								open = true;
							}}
						>
							Edit
						</Button>
						<form method="POST" action="?/archive" use:enhance={done('Work type archived.')}>
							<input type="hidden" name="id" value={w.id} />
							<Button size="sm" type="submit" variant="ghost">Archive</Button>
						</form>
					</div>
				</li>
			{/each}
		</ul>
	</Card>
</div>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New work type'}>
	<form
		method="POST"
		action="?/save"
		class="flex flex-col gap-4"
		use:enhance={done('Work type saved.', () => (open = false))}
	>
		{#if editing}
			<input type="hidden" name="id" value={editing.id} />
			<input type="hidden" name="version" value={editing.version} />
		{/if}
		<Field label="Name" for="work-type-name" error={errors.name}>
			<Input id="work-type-name" name="name" required maxlength={60} value={editing?.name ?? ''} />
		</Field>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" name="billable" checked={editing?.billable ?? true} />
			Billable unless said otherwise
		</label>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" name="onTickets" checked={editing?.onTickets ?? true} />
			Offered on tickets
		</label>
		<div class="flex justify-end gap-2">
			<Button onclick={() => (open = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>
