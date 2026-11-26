<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus } from '@lucide/svelte';
	import {
		Badge,
		Button,
		Card,
		CodeEditor,
		Dialog,
		Field,
		Input,
		Select,
		toast
	} from '@usehuxley/ui';
	import { byCategory } from '#lib/workforce/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	type Response = (typeof data.responses)[number];
	let open = $state(false);
	let editing: Response | null = $state(null);
	let errors: Record<string, string> = $derived(form?.errors ?? {});
	let groups = $derived(byCategory(data.responses));
	let categories = $derived([...new Set(data.responses.map((r) => r.category).filter(Boolean))]);

	const PLACEHOLDERS = [
		['requester.first_name', "The requester's first name"],
		['requester.name', "The requester's full name"],
		['ticket.reference', "The ticket's reference"],
		['ticket.title', "The ticket's title"],
		['client.name', "The ticket's organization"],
		['me.name', 'Your name'],
		['me.first_name', 'Your first name']
	] as const;
	const KINDS = [
		{ value: 'public', label: 'Reply to the requester' },
		{ value: 'internal', label: 'Internal note' }
	];

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
	<title>Canned responses · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Canned responses"
		description="Replies and notes technicians insert on a ticket, filled in for the ticket and themselves."
	>
		{#snippet actions()}
			<Button
				onclick={() => {
					editing = null;
					open = true;
				}}
			>
				<Plus size={16} /> New response
			</Button>
		{/snippet}
		{#if data.responses.length === 0}
			<p class="text-sm text-fg-muted">No responses yet.</p>
		{:else}
			<div class="flex flex-col gap-4">
				{#each groups as [category, responses] (category)}
					<section>
						<h3 class="text-xs font-medium tracking-wide text-fg-muted uppercase">{category}</h3>
						<ul class="mt-1 flex flex-col divide-y divide-border" aria-label={category}>
							{#each responses as r (r.id)}
								<li class="flex items-center justify-between gap-3 py-2">
									<div class="min-w-0">
										<p class="flex items-center gap-2 text-sm text-fg">
											{r.name}
											{#if r.kind === 'internal'}<Badge>Internal</Badge>{/if}
										</p>
										<p class="truncate text-xs text-fg-muted">{r.body}</p>
									</div>
									<div class="flex shrink-0 gap-2">
										<Button
											size="sm"
											onclick={() => {
												editing = r;
												open = true;
											}}
										>
											Edit
										</Button>
										<form method="POST" action="?/archive" use:enhance={done('Response archived.')}>
											<input type="hidden" name="id" value={r.id} />
											<Button size="sm" type="submit" variant="ghost">Archive</Button>
										</form>
									</div>
								</li>
							{/each}
						</ul>
					</section>
				{/each}
			</div>
		{/if}
	</Card>
</div>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New response'}>
	<form
		method="POST"
		action="?/save"
		class="flex flex-col gap-4"
		use:enhance={done('Response saved.', () => (open = false))}
	>
		{#if editing}
			<input type="hidden" name="id" value={editing.id} />
			<input type="hidden" name="version" value={editing.version} />
		{/if}
		<div class="grid gap-4 sm:grid-cols-2">
			<Field label="Name" for="response-name" error={errors.name}>
				<Input
					id="response-name"
					name="name"
					required
					maxlength={100}
					value={editing?.name ?? ''}
				/>
			</Field>
			<Field label="Category" for="response-category" hint="e.g. Accounts, Hardware">
				<Input
					id="response-category"
					name="category"
					maxlength={100}
					list="response-categories"
					value={editing?.category ?? ''}
				/>
				<datalist id="response-categories">
					{#each categories as c (c)}<option value={c}></option>{/each}
				</datalist>
			</Field>
		</div>
		<Field label="Inserted as" for="response-kind" error={errors.kind}>
			<Select id="response-kind" name="kind" value={editing?.kind ?? 'public'} options={KINDS} />
		</Field>
		<Field label="Text" for="response-body" error={errors.body}>
			<CodeEditor
				id="response-body"
				name="body"
				label="Text"
				language="jinja"
				lines={10}
				value={editing?.body ?? ''}
				words={PLACEHOLDERS.map(([key, label]) => ({ label: key, detail: label }))}
			/>
		</Field>
		<details class="text-sm text-fg-muted">
			<summary class="cursor-pointer">Placeholders</summary>
			<ul class="mt-2 flex flex-col gap-1">
				{#each PLACEHOLDERS as [key, label] (key)}
					<li><code class="font-mono text-fg">{`{{${key}}}`}</code> · {label}</li>
				{/each}
			</ul>
		</details>
		<div class="flex justify-end gap-2">
			<Button onclick={() => (open = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>
