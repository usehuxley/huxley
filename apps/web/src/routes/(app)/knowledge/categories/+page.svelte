<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Plus } from '@lucide/svelte';
	import { Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	type Category = (typeof data.categories)[number];
	let open = $state(false);
	let editing: Category | null = $state(null);
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
	<title>Categories · Knowledge · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<header>
		<p class="text-sm">
			<a class="text-fg-muted hover:text-fg" href={resolve('/(app)/knowledge')}>Knowledge</a>
		</p>
		<h1 class="text-2xl font-semibold text-fg">Categories</h1>
	</header>
	<Card
		title="Categories"
		description="How articles are grouped. Articles at an organization and below can use its categories."
	>
		{#snippet actions()}
			<Button
				onclick={() => {
					editing = null;
					open = true;
				}}
			>
				<Plus size={16} /> New category
			</Button>
		{/snippet}
		{#if data.categories.length === 0}
			<p class="text-sm text-fg-muted">None yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Categories">
				{#each data.categories as c (c.id)}
					<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
						<div class="min-w-0">
							<p class="text-sm text-fg">{c.name}</p>
							<p class="text-xs text-fg-subtle">
								{c.orgName} · {c.articles} published{c.description ? ` · ${c.description}` : ''}
							</p>
						</div>
						<div class="flex shrink-0 gap-2">
							<Button
								size="sm"
								onclick={() => {
									editing = c;
									open = true;
								}}
							>
								Edit
							</Button>
							<form method="POST" action="?/delete" use:enhance={done('Category deleted.')}>
								<input type="hidden" name="id" value={c.id} />
								<Button size="sm" type="submit" variant="ghost">Delete</Button>
							</form>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>

<Dialog bind:open title={editing ? 'Edit category' : 'New category'}>
	<form
		method="POST"
		action="?/save"
		use:enhance={done('Category saved.', () => (open = false))}
		class="flex flex-col gap-4"
	>
		{#if editing}
			<input type="hidden" name="id" value={editing.id} />
		{:else}
			<Field label="Organization" for="cat-org">
				<Select id="cat-org" name="orgId" options={data.orgs} />
			</Field>
		{/if}
		<Field label="Name" for="cat-name" error={errors.name}>
			<Input id="cat-name" name="name" required maxlength={100} value={editing?.name ?? ''} />
		</Field>
		<Field label="Description" for="cat-description" error={errors.description}>
			<Input
				id="cat-description"
				name="description"
				maxlength={500}
				value={editing?.description ?? ''}
			/>
		</Field>
		<Field label="Position" for="cat-position" hint="Lower comes first">
			<Input
				id="cat-position"
				name="position"
				type="number"
				value={String(editing?.position ?? 0)}
			/>
		</Field>
		<div class="flex justify-end gap-2">
			<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>
