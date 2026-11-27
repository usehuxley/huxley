<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import {
		Badge,
		Button,
		Card,
		Dialog,
		EmptyState,
		Field,
		Input,
		Select,
		toast
	} from '@usehuxley/ui';
	import { Folder, FolderPlus, Lock, Pencil, Trash2, Workflow } from '@lucide/svelte';
	import type { components } from '@usehuxley/api-client';
	import type { PageProps } from './$types';

	type WorkflowFolder = components['schemas']['WorkflowFolder'];

	let { data, form }: PageProps = $props();
	let canWrite = $derived(data.user.grantedPermissions.includes('workflow:write'));
	let orgId = $state('');
	$effect(() => {
		if (!orgId) orgId = data.orgs[0]?.id ?? '';
	});
	let orgOptions = $derived(data.orgs.map((o) => ({ value: o.id, label: o.name })));
	let orgName = $derived(new Map(data.orgs.map((o) => [o.id, o.name])));

	// The folder tree, and which part of it is shown.
	let byId = $derived(new Map(data.folders.map((f) => [f.id, f])));
	let selected: string = $state('all'); // 'all', 'unfiled', or a folder's id
	let folder = $derived(byId.get(selected));
	let treeOrgs = $derived([...new Set(data.folders.map((f) => f.orgId))]);
	const children = (parent: string | null, org: string) =>
		data.folders.filter((f) => f.parentId === parent && f.orgId === org);
	function path(f: WorkflowFolder): string {
		const parent = f.parentId ? byId.get(f.parentId) : undefined;
		return parent ? `${path(parent)} / ${f.name}` : f.name;
	}

	// Search and status cover every folder.
	let query = $state('');
	let status = $state('');
	let searching = $derived(query.trim() !== '' || status !== '');
	let shown = $derived(
		data.workflows.filter((w) => {
			if (searching) {
				const q = query.trim().toLowerCase();
				if (q && !w.name.toLowerCase().includes(q) && !w.key.includes(q)) return false;
				if (status === 'published' && !(w.status === 'active' && w.currentVersion)) return false;
				if (status === 'draft' && !(w.status === 'active' && !w.currentVersion)) return false;
				if (status === 'archived' && w.status !== 'archived') return false;
				return true;
			}
			if (selected === 'all') return true;
			if (selected === 'unfiled') return !w.folderId;
			return w.folderId === selected;
		})
	);
	let unfiled = $derived(data.workflows.filter((w) => !w.folderId).length);

	// Moving: choose workflows, then a folder of their organization.
	let choosing = $state(false);
	let chosen: string[] = $state([]);
	let chosenOrgs = $derived(
		new Set(data.workflows.filter((w) => chosen.includes(w.id)).map((w) => w.orgId))
	);
	let moveTo = $state('');
	let moveOptions = $derived([
		{ value: '', label: 'Unfiled (no folder)' },
		...data.folders
			.filter((f) => !f.system && (chosenOrgs.size !== 1 || chosenOrgs.has(f.orgId)))
			.map((f) => ({
				value: f.id,
				label: treeOrgs.length > 1 ? `${path(f)} (${orgName.get(f.orgId) ?? ''})` : path(f)
			}))
	]);

	// Making, renaming and moving folders.
	let folderOpen = $state(false);
	let editing: WorkflowFolder | null = $state(null);
	let folderName = $state('');
	let folderOrg = $state('');
	let folderParent = $state('');
	let parentOptions = $derived([
		{ value: '', label: 'Top level' },
		...data.folders
			.filter(
				(f) =>
					!f.system &&
					f.orgId === folderOrg &&
					f.id !== editing?.id &&
					!(editing && path(f).startsWith(`${path(editing)} / `))
			)
			.map((f) => ({ value: f.id, label: path(f) }))
	]);
	function newFolder() {
		editing = null;
		folderName = '';
		folderOrg = folder?.orgId ?? orgId;
		folderParent = folder && !folder.system ? folder.id : '';
		folderOpen = true;
	}
	function editFolder(f: WorkflowFolder) {
		editing = f;
		folderName = f.name;
		folderOrg = f.orgId;
		folderParent = f.parentId ?? '';
		folderOpen = true;
	}

	const done =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				const message = (result.data as { done?: string } | undefined)?.done ?? 'Done.';
				toast.push({ title: message, tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>Workflows · Huxley</title>
</svelte:head>

{#snippet tree(parent: string | null, org: string, depth: number)}
	{#each children(parent, org) as f (f.id)}
		<li>
			<button
				type="button"
				class="flex w-full items-center gap-2 rounded-md py-1.5 pr-2 text-left text-sm hover:bg-surface-sunken {selected ===
				f.id
					? 'bg-surface-sunken font-medium text-fg'
					: 'text-fg-muted'}"
				style="padding-left: {0.5 + depth * 1}rem"
				aria-current={selected === f.id ? 'true' : undefined}
				onclick={() => (selected = f.id)}
			>
				{#if f.system}<Lock size={14} aria-hidden="true" />{:else}<Folder
						size={14}
						aria-hidden="true"
					/>{/if}
				<span class="min-w-0 flex-1 truncate">{f.name}</span>
				<span class="text-xs text-fg-subtle">{f.workflows}</span>
			</button>
			<ul>{@render tree(f.id, org, depth + 1)}</ul>
		</li>
	{/each}
{/snippet}

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<header>
		<h1 class="text-2xl font-semibold text-fg">Workflows</h1>
		<p class="mt-1 text-sm text-fg-muted">
			Automations that run on a schedule, from a webhook, or when someone starts them.
			<a href={resolve('/(app)/runs')} class="text-accent hover:underline">All runs</a>
		</p>
	</header>

	<div class="flex flex-col gap-6 md:flex-row md:items-start">
		<nav aria-label="Folders" class="flex shrink-0 flex-col gap-2 md:w-60">
			<ul class="flex flex-col">
				<li>
					<button
						type="button"
						class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-surface-sunken {selected ===
						'all'
							? 'bg-surface-sunken font-medium text-fg'
							: 'text-fg-muted'}"
						aria-current={selected === 'all' ? 'true' : undefined}
						onclick={() => (selected = 'all')}
					>
						<span class="flex-1">All workflows</span>
						<span class="text-xs text-fg-subtle">{data.workflows.length}</span>
					</button>
				</li>
				<li>
					<button
						type="button"
						class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-surface-sunken {selected ===
						'unfiled'
							? 'bg-surface-sunken font-medium text-fg'
							: 'text-fg-muted'}"
						aria-current={selected === 'unfiled' ? 'true' : undefined}
						onclick={() => (selected = 'unfiled')}
					>
						<span class="flex-1">Unfiled</span>
						<span class="text-xs text-fg-subtle">{unfiled}</span>
					</button>
				</li>
			</ul>
			{#each treeOrgs as org (org)}
				<div>
					{#if treeOrgs.length > 1}
						<p class="px-2 pt-2 pb-1 text-xs font-medium tracking-wide text-fg-subtle uppercase">
							{orgName.get(org) ?? 'Organization'}
						</p>
					{/if}
					<ul class="flex flex-col">{@render tree(null, org, 0)}</ul>
				</div>
			{/each}
			{#if canWrite}
				<div>
					<Button size="sm" variant="secondary" onclick={newFolder}>
						<FolderPlus size={14} /> New folder
					</Button>
				</div>
			{/if}
		</nav>

		<div class="min-w-0 flex-1">
			<Card
				title={searching
					? 'Search results (all folders)'
					: selected === 'all'
						? 'All workflows'
						: selected === 'unfiled'
							? 'Unfiled'
							: (folder?.name ?? 'Folder')}
				description={folder?.system && !searching
					? 'Workflows from Huxley’s built-in packs. They stay here, and this folder can’t be changed.'
					: undefined}
			>
				{#snippet actions()}
					{#if canWrite && folder && !folder.system && !searching}
						<Button size="sm" variant="ghost" onclick={() => editFolder(folder)}>
							<Pencil size={14} /> Rename or move
						</Button>
						<form
							method="POST"
							action="?/deleteFolder"
							use:enhance={done(() => (selected = 'all'))}
						>
							<input type="hidden" name="id" value={folder.id} />
							<Button size="sm" variant="ghost" type="submit">
								<Trash2 size={14} /> Delete folder
							</Button>
						</form>
					{/if}
					{#if canWrite && data.workflows.length > 0}
						<Button
							size="sm"
							variant={choosing ? 'secondary' : 'ghost'}
							onclick={() => {
								choosing = !choosing;
								chosen = [];
							}}
						>
							{choosing ? 'Done moving' : 'Move workflows'}
						</Button>
					{/if}
				{/snippet}
				<div class="mb-3 flex flex-wrap items-end gap-3">
					<div class="w-64">
						<Field label="Search workflows" for="wf-search">
							<Input id="wf-search" type="search" bind:value={query} placeholder="Name or key" />
						</Field>
					</div>
					<div class="w-44">
						<Field label="Status" for="wf-status">
							<Select
								id="wf-status"
								bind:value={status}
								options={[
									{ value: '', label: 'Any' },
									{ value: 'published', label: 'Published' },
									{ value: 'draft', label: 'Draft only' },
									{ value: 'archived', label: 'Archived' }
								]}
							/>
						</Field>
					</div>
				</div>
				{#if choosing}
					<form
						method="POST"
						action="?/move"
						class="mb-3 flex flex-wrap items-end gap-3 rounded-md border border-border p-3"
						use:enhance={done(() => (chosen = []))}
					>
						{#each chosen as id (id)}<input type="hidden" name="workflowId" value={id} />{/each}
						<p class="text-sm text-fg-muted">
							{chosen.length} chosen{chosenOrgs.size > 1 ? ', from several organizations' : ''}
						</p>
						<div class="w-64">
							<Field label="Move to" for="wf-move-to">
								<Select id="wf-move-to" name="folderId" bind:value={moveTo} options={moveOptions} />
							</Field>
						</div>
						<Button type="submit" disabled={chosen.length === 0}>Move</Button>
					</form>
				{/if}
				{#if shown.length === 0}
					<EmptyState
						title={data.workflows.length === 0 ? 'No workflows yet' : 'Nothing here'}
						description={data.workflows.length === 0
							? 'Create one below to automate a task.'
							: searching
								? 'No workflow matches.'
								: 'This folder has no workflows.'}
					>
						{#snippet icon()}<Workflow size={20} />{/snippet}
					</EmptyState>
				{:else}
					<ul class="flex flex-col divide-y divide-border" aria-label="Workflows">
						{#each shown as workflow (workflow.id)}
							{@const inFolder = workflow.folderId ? byId.get(workflow.folderId) : undefined}
							<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
								<div class="flex min-w-0 items-center gap-3">
									{#if choosing}
										<input
											type="checkbox"
											aria-label="Choose {workflow.name}"
											disabled={workflow.system}
											title={workflow.system ? 'Built-in workflows stay in the System folder' : ''}
											checked={chosen.includes(workflow.id)}
											onchange={(e) => {
												chosen = e.currentTarget.checked
													? [...chosen, workflow.id]
													: chosen.filter((c) => c !== workflow.id);
											}}
										/>
									{/if}
									<div class="min-w-0">
										<a
											href={resolve('/(app)/workflows/[id]', { id: workflow.id })}
											class="text-sm font-medium text-fg hover:underline"
										>
											{workflow.name}
										</a>
										<p class="mt-0.5 font-mono text-xs text-fg-muted">
											{workflow.key} · {orgName.get(workflow.orgId) ??
												'Unknown organization'}{#if inFolder && (selected === 'all' || searching)}
												· {path(inFolder)}{/if}
										</p>
									</div>
								</div>
								<div class="flex items-center gap-2">
									{#if workflow.status === 'archived'}
										<Badge tone="neutral">Archived</Badge>
									{:else if workflow.currentVersion}
										<Badge tone="success">Version {workflow.currentVersion}</Badge>
									{:else}
										<Badge tone="warning">Draft</Badge>
									{/if}
								</div>
							</li>
						{/each}
					</ul>
				{/if}
			</Card>
		</div>
	</div>

	{#if canWrite}
		<Card title="New workflow" description="Starts as a draft with one step to show the shape.">
			<form method="POST" action="?/create" class="flex flex-wrap items-end gap-3" use:enhance>
				<Field label="Name" for="name">
					<Input id="name" name="name" required maxlength={200} />
				</Field>
				<Field label="Key" for="key" hint="Optional; made from the name">
					<Input id="key" name="key" pattern="[a-z][a-z0-9-]*" maxlength={64} />
				</Field>
				<Field label="Organization" for="orgId">
					<Select id="orgId" name="orgId" options={orgOptions} bind:value={orgId} />
				</Field>
				<Button type="submit">Create</Button>
			</form>
			{#if form && 'message' in form && form.message}<p
					class="mt-3 text-sm text-danger"
					role="alert"
				>
					{form.message}
				</p>{/if}
			<p class="mt-3 text-sm text-fg-muted">
				Or <a href={resolve('/(app)/workflows/templates')} class="text-accent hover:underline">
					start from a template
				</a>.
			</p>
		</Card>
	{/if}
</div>

<Dialog bind:open={folderOpen} title={editing ? `Rename or move ${editing.name}` : 'New folder'}>
	<form
		method="POST"
		action={editing ? '?/updateFolder' : '?/createFolder'}
		class="flex flex-col gap-4"
		use:enhance={done(() => (folderOpen = false))}
	>
		{#if editing}<input type="hidden" name="id" value={editing.id} />{/if}
		<Field label="Folder name" for="folder-name">
			<Input id="folder-name" name="folderName" bind:value={folderName} required maxlength={100} />
		</Field>
		{#if !editing && data.orgs.length > 1}
			<Field label="Organization" for="folder-org">
				<Select
					id="folder-org"
					name="orgId"
					options={orgOptions}
					bind:value={folderOrg}
					onchange={() => (folderParent = '')}
				/>
			</Field>
		{:else if !editing}
			<input type="hidden" name="orgId" value={folderOrg} />
		{/if}
		<Field label="Inside" for="folder-parent">
			<Select
				id="folder-parent"
				name="parentId"
				options={parentOptions}
				bind:value={folderParent}
			/>
		</Field>
		<div class="flex justify-end gap-2">
			<Button variant="ghost" onclick={() => (folderOpen = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>
