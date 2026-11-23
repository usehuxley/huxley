<script lang="ts">
	import { resolve } from '$app/paths';
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Building2, FolderTree } from '@lucide/svelte';
	import { Badge, Button, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import type { PageProps } from './$types';

	type Org = components['schemas']['Organization'];
	type Mode = 'create' | 'rename' | 'move' | 'archive';

	let { data }: PageProps = $props();

	let open = $state(false);
	let mode = $state<Mode>('create');
	let target = $state<Org | null>(null);
	let kind = $state('client');
	let newParent = $state('');
	let dialogError = $state('');

	let canManage = $derived(data.user.grantedPermissions.includes('org:create'));
	let active = $derived(data.orgs.filter((o) => o.status === 'active'));

	const titles: Record<Mode, string> = {
		create: 'Add sub-organization',
		rename: 'Rename organization',
		move: 'Move organization',
		archive: 'Archive organization'
	};

	function openDialog(next: Mode, org: Org) {
		mode = next;
		target = org;
		kind = 'client';
		dialogError = '';
		newParent = '';
		open = true;
	}

	/** Orgs that can be a new parent: active, not the org itself or its descendants. */
	function moveTargets(org: Org) {
		const blocked: string[] = [org.id];
		for (const o of data.orgs) {
			if (o.parentId && blocked.includes(o.parentId)) blocked.push(o.id);
		}
		return active
			.filter((o) => !blocked.includes(o.id) && o.id !== org.parentId)
			.map((o) => ({ value: o.id, label: `${'— '.repeat(o.depth - 1)}${o.name}` }));
	}

	const onResult: SubmitFunction = () => {
		dialogError = '';
		return async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				open = false;
				toast.push({ title: String(result.data?.message ?? 'Saved'), tone: 'success' });
			} else if (result.type === 'failure') {
				dialogError = String(result.data?.message ?? 'Something went wrong. Try again.');
			}
		};
	};
</script>

<svelte:head>
	<title>Organizations · Settings · Huxley</title>
</svelte:head>

<section class="rounded-card border border-border bg-surface">
	<header class="flex items-center gap-2 border-b border-border px-5 py-3">
		<FolderTree size={18} class="text-fg-muted" />
		<h2 class="text-sm font-semibold text-fg">Organization tree</h2>
		<span class="text-sm text-fg-muted">· {active.length} active</span>
	</header>

	<ul class="divide-y divide-border">
		{#each data.orgs as org (org.id)}
			<li
				class="flex flex-wrap items-center gap-3 py-2.5 pr-4"
				style="padding-left: {org.depth * 1.25}rem"
			>
				<Building2 size={16} class="shrink-0 text-fg-subtle" />
				<a
					href={resolve('/(app)/settings/organizations/[id]', { id: org.id })}
					class="font-medium text-fg hover:underline {org.status === 'archived'
						? 'line-through opacity-60'
						: ''}"
				>
					{org.name}
				</a>
				<Badge tone={org.kind === 'provider' ? 'accent' : 'neutral'}>{org.kind}</Badge>
				{#if org.status === 'archived'}<Badge tone="warning">archived</Badge>{/if}
				{#if canManage && org.status === 'active'}
					<div class="ml-auto flex gap-1">
						<Button size="sm" variant="ghost" onclick={() => openDialog('create', org)}>Add</Button>
						<Button size="sm" variant="ghost" onclick={() => openDialog('rename', org)}>
							Rename
						</Button>
						{#if org.parentId}
							<Button size="sm" variant="ghost" onclick={() => openDialog('move', org)}>
								Move
							</Button>
							<Button size="sm" variant="ghost" onclick={() => openDialog('archive', org)}>
								Archive
							</Button>
						{/if}
					</div>
				{/if}
			</li>
		{/each}
	</ul>
</section>

<Dialog bind:open title={titles[mode]} description={target ? target.name : undefined}>
	{#if target}
		<form method="POST" action="?/{mode}" use:enhance={onResult} class="flex flex-col gap-4">
			<input type="hidden" name="id" value={target.id} />
			<input type="hidden" name="version" value={target.version} />
			{#if dialogError}
				<p
					role="alert"
					class="rounded-control border border-danger/30 bg-danger-soft px-3 py-2 text-sm text-danger"
				>
					{dialogError}
				</p>
			{/if}

			{#if mode === 'create'}
				<input type="hidden" name="parentId" value={target.id} />
				<Field label="Name" for="org-name" required>
					<Input id="org-name" name="name" required maxlength={200} />
				</Field>
				<Field
					label="Type"
					for="org-kind"
					hint="Clients are customers. Providers are MSP brands or divisions."
				>
					<Select
						id="org-kind"
						name="kind"
						bind:value={kind}
						options={target.kind === 'client'
							? [{ value: 'client', label: 'Client' }]
							: [
									{ value: 'client', label: 'Client' },
									{ value: 'provider', label: 'Provider' }
								]}
					/>
				</Field>
			{:else if mode === 'rename'}
				<Field label="Name" for="org-rename" required>
					<Input id="org-rename" name="name" required maxlength={200} value={target.name} />
				</Field>
			{:else if mode === 'move'}
				<Field label="New parent" for="org-parent" hint="Its sub-organizations move with it.">
					<Select
						id="org-parent"
						name="parentId"
						required
						placeholder="Choose an organization"
						bind:value={newParent}
						options={moveTargets(target)}
					/>
				</Field>
			{:else}
				<p class="text-sm text-fg-muted">
					Archived organizations are hidden from day-to-day use but keep their history. It must have
					no active sub-organizations.
				</p>
			{/if}

			<div class="flex justify-end gap-2">
				<Button onclick={() => (open = false)}>Cancel</Button>
				<Button type="submit" variant={mode === 'archive' ? 'danger' : 'primary'}>
					{mode === 'create' ? 'Create' : mode === 'archive' ? 'Archive' : 'Save'}
				</Button>
			</div>
		</form>
	{/if}
</Dialog>
