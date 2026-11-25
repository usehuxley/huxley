<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Plus } from '@lucide/svelte';
	import { Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import { LEVELS } from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	type Template = (typeof data.templates)[number];
	let open = $state(false);
	let editing: Template | null = $state(null);
	let errors: Record<string, string> = $derived(form?.errors ?? {});
	let typeOptions = $derived(data.types.map((t) => ({ value: t.key, label: t.name })));
	let queueOptions = $derived([
		{ value: '', label: 'None' },
		...data.queues.map((q) => ({ value: q.id, label: q.name }))
	]);
	const textarea =
		'w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg disabled:opacity-50';

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
	<title>Ticket templates · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Ticket templates"
		description="Common tickets, set up once: choose one on New ticket and its type, title, description, impact, urgency and queue are filled in."
	>
		{#snippet actions()}
			<Button
				onclick={() => {
					editing = null;
					open = true;
				}}
			>
				<Plus size={16} /> New template
			</Button>
		{/snippet}
		{#if data.templates.length === 0}
			<p class="text-sm text-fg-muted">No templates yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Templates">
				{#each data.templates as tp (tp.id)}
					<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
						<div class="min-w-0">
							<p class="text-sm text-fg">{tp.name}</p>
							<p class="truncate text-xs text-fg-muted">
								{data.types.find((t) => t.key === tp.typeKey)?.name ?? tp.typeKey}{tp.queueName
									? ` · ${tp.queueName}`
									: ''}{tp.title ? ` · “${tp.title}”` : ''}
							</p>
						</div>
						<div class="flex shrink-0 gap-2">
							<!-- eslint-disable svelte/no-navigation-without-resolve -- resolve() plus a query string -->
							<Button size="sm" href={resolve('/(app)/tickets/new') + `?template=${tp.id}`}>
								Use
							</Button>
							<!-- eslint-enable svelte/no-navigation-without-resolve -->
							<Button
								size="sm"
								onclick={() => {
									editing = tp;
									open = true;
								}}
							>
								Edit
							</Button>
							<form method="POST" action="?/archive" use:enhance={done('Template archived.')}>
								<input type="hidden" name="id" value={tp.id} />
								<Button size="sm" type="submit" variant="ghost">Archive</Button>
							</form>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New template'}>
	<form
		method="POST"
		action="?/save"
		class="flex flex-col gap-4"
		use:enhance={done('Template saved.', () => (open = false))}
	>
		{#if editing}
			<input type="hidden" name="id" value={editing.id} />
			<input type="hidden" name="version" value={editing.version} />
		{/if}
		<Field label="Name" for="template-name" error={errors.name}>
			<Input
				id="template-name"
				name="name"
				required
				maxlength={100}
				value={editing?.name ?? ''}
				placeholder="e.g. New starter"
			/>
		</Field>
		<div class="grid gap-4 sm:grid-cols-2">
			<Field label="Type" for="template-type" error={errors.typeKey}>
				<Select
					id="template-type"
					name="typeKey"
					value={editing?.typeKey ?? 'incident'}
					options={typeOptions}
				/>
			</Field>
			<Field label="Queue" for="template-queue" error={errors.queueId}>
				<Select
					id="template-queue"
					name="queueId"
					value={editing?.queueId ?? ''}
					options={queueOptions}
				/>
			</Field>
			<Field label="Impact" for="template-impact" error={errors.impact}>
				<Select
					id="template-impact"
					name="impact"
					value={String(editing?.impact ?? 2)}
					options={[...LEVELS]}
				/>
			</Field>
			<Field label="Urgency" for="template-urgency" error={errors.urgency}>
				<Select
					id="template-urgency"
					name="urgency"
					value={String(editing?.urgency ?? 2)}
					options={[...LEVELS]}
				/>
			</Field>
		</div>
		<Field label="Title" for="template-title" error={errors.title} hint="Finish it on the ticket.">
			<Input
				id="template-title"
				name="title"
				maxlength={300}
				value={editing?.title ?? ''}
				placeholder="e.g. New starter: "
			/>
		</Field>
		<Field label="Description" for="template-description" error={errors.description}>
			<textarea
				id="template-description"
				name="description"
				rows="6"
				class={textarea}
				value={editing?.description ?? ''}></textarea>
		</Field>
		<div class="flex justify-end gap-2">
			<Button onclick={() => (open = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>
