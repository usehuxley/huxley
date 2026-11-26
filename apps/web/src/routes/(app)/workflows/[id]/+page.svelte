<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import {
		Badge,
		Button,
		Card,
		CodeEditor,
		Field,
		Input,
		Select,
		Tabs,
		toast
	} from '@usehuxley/ui';
	import { lintJson, lintWorkflowYaml } from '#lib/workflows/yaml-lint.ts';
	import RunStatusBadge from '#lib/workflows/RunStatusBadge.svelte';
	import VisualEditor from '#lib/workflows/editor/VisualEditor.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let workflow = $derived(data.workflow);
	let granted = $derived(data.user.grantedPermissions);
	let canWrite = $derived(granted.includes('workflow:write'));
	let canPublish = $derived(granted.includes('workflow:publish'));
	async function previewExpression(expression: unknown) {
		const response = await fetch(resolve('/(app)/workflows/[id]/preview', { id: workflow.id }), {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ expression })
		});
		return (await response.json()) as { value?: unknown; error?: string | null };
	}
	async function checkDocument(document: unknown) {
		const response = await fetch(resolve('/(app)/workflows/[id]/check', { id: workflow.id }), {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ document })
		});
		if (!response.ok) return [];
		return ((await response.json()) as { problems: { path: string; message: string }[] }).problems;
	}
	let canRun = $derived(granted.includes('workflow:run'));
	let archived = $derived(workflow.status === 'archived');
	let canAssignRunAs = $derived(canPublish && granted.includes('service_account:manage'));
	let runAsOptions = $derived([
		{ value: '', label: 'No identity' },
		...data.serviceAccounts.map((a) => ({ value: a.id, label: a.name }))
	]);

	// Keep unsaved edits when a save fails; take the server's copy otherwise.
	let yaml = $state('');
	let edited = $state(false);
	$effect(() => {
		if (form && 'yaml' in form && typeof form.yaml === 'string') yaml = form.yaml;
		else if (!edited) yaml = data.yaml;
	});
	let inputs = $state('{}');

	// Visual and YAML edit the same draft; switching needs the other saved.
	let tab = $state('visual');
	let visualDirty = $state(false);
	let visualDocument = $state<Record<string, unknown>>();
	let tabs = $derived([
		{ value: 'visual', label: 'Visual', disabled: edited },
		{ value: 'yaml', label: 'YAML', disabled: visualDirty }
	]);

	const notify: SubmitFunction = () => {
		return async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				edited = false;
				visualDirty = false;
			}
			if ((result.type === 'success' || result.type === 'failure') && result.data?.message) {
				toast.push({
					title: String(result.data.message),
					tone: result.type === 'success' ? 'success' : 'danger'
				});
			}
		};
	};

	function when(value: string | null | undefined): string {
		return value ? new Date(value).toLocaleString() : '';
	}
</script>

<svelte:head>
	<title>{workflow.name} · Workflows · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<a
		href={resolve('/(app)/workflows')}
		class="flex w-fit items-center gap-1 text-sm text-fg-muted hover:text-fg"
	>
		<ArrowLeft size={16} /> All workflows
	</a>

	<header class="flex flex-wrap items-start justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-fg">{workflow.name}</h1>
			<p class="mt-1 font-mono text-xs text-fg-muted">{workflow.key}</p>
			{#if workflow.description}<p class="mt-2 text-sm text-fg-muted">
					{workflow.description}
				</p>{/if}
		</div>
		<div class="flex items-center gap-2">
			{#if archived}
				<Badge tone="neutral">Archived</Badge>
			{:else if workflow.currentVersion}
				<Badge tone="success">Version {workflow.currentVersion} published</Badge>
			{:else}
				<Badge tone="warning">Not published</Badge>
			{/if}
			{#if canPublish}
				<form method="POST" action={archived ? '?/restore' : '?/archive'} use:enhance={notify}>
					<Button type="submit" variant="secondary" size="sm">
						{archived ? 'Restore' : 'Archive'}
					</Button>
				</form>
			{/if}
		</div>
	</header>

	<Card
		title="Definition"
		description="The draft, in the Huxley Workflow Format. Saving keeps it as a draft; publishing makes it the version that runs."
	>
		<Tabs items={tabs} bind:value={tab} label="Editor">
			{#snippet panel(which)}
				{#if which === 'visual'}
					<form
						method="POST"
						action="?/saveDocument"
						class="flex flex-col gap-3"
						use:enhance={notify}
					>
						<input type="hidden" name="revision" value={workflow.revision} />
						<input type="hidden" name="document" value={JSON.stringify(visualDocument ?? {})} />
						<VisualEditor
							preview={canWrite ? previewExpression : undefined}
							check={canWrite ? checkDocument : undefined}
							draft={workflow.draft}
							diagnostics={workflow.diagnostics}
							readonly={!canWrite || archived}
							bind:dirty={visualDirty}
							bind:document={visualDocument}
							actions={data.actions}
							modules={data.user.activeModules}
						/>
						{#if canWrite && !archived}
							<div class="flex items-center gap-3">
								<Button type="submit">Save draft</Button>
								{#if visualDirty}<span class="text-sm text-fg-muted">Unsaved changes</span>{/if}
							</div>
						{/if}
					</form>
				{:else}
					<form method="POST" action="?/save" class="flex flex-col gap-3" use:enhance={notify}>
						<input type="hidden" name="revision" value={workflow.revision} />
						<CodeEditor
							id="yaml"
							name="yaml"
							label="Workflow YAML"
							language="yaml"
							lines={36}
							bind:value={yaml}
							onchange={() => (edited = true)}
							readonly={!canWrite || archived}
							lint={canWrite ? (text: string) => lintWorkflowYaml(text, checkDocument) : undefined}
						/>
						{#if workflow.diagnostics.length > 0}
							<div class="rounded-control border border-warning/40 bg-warning/5 p-3" role="status">
								<p class="text-sm font-medium text-fg">
									{workflow.diagnostics.length} problem{workflow.diagnostics.length === 1
										? ''
										: 's'} to fix before publishing
								</p>
								<ul class="mt-2 flex flex-col gap-1" aria-label="Problems">
									{#each workflow.diagnostics as problem, i (i)}
										<li class="text-sm text-fg-muted">
											{#if problem.path}<code class="text-fg">{problem.path}</code>:
											{/if}{problem.message}
										</li>
									{/each}
								</ul>
							</div>
						{/if}
						{#if canWrite && !archived}
							<div class="flex items-center gap-3">
								<Button type="submit">Save draft</Button>
								{#if edited}<span class="text-sm text-fg-muted">Unsaved changes</span>{/if}
							</div>
						{/if}
					</form>
				{/if}
			{/snippet}
		</Tabs>
		{#if canPublish && !archived}
			<form
				method="POST"
				action="?/publish"
				class="mt-4 flex flex-wrap items-end gap-3 border-t border-border pt-4"
				use:enhance={notify}
			>
				<Field label="Release notes" for="notes" hint="Optional">
					<Input id="notes" name="notes" maxlength={500} />
				</Field>
				<Button
					type="submit"
					variant="secondary"
					disabled={edited || visualDirty || workflow.diagnostics.length > 0}
				>
					Publish version {(workflow.currentVersion ?? 0) + 1}
				</Button>
				{#if edited || visualDirty}<span class="text-sm text-fg-muted">Save first</span>{/if}
			</form>
		{/if}
	</Card>

	{#if !archived && (canRun || canWrite)}
		<Card title="Run" description="Inputs as JSON. A test run uses the saved draft.">
			<form method="POST" action="?/run" class="flex flex-col gap-3" use:enhance>
				<CodeEditor
					id="inputs"
					name="inputs"
					label="Inputs"
					language="json"
					lines={6}
					bind:value={inputs}
					lint={lintJson}
				/>
				<div class="flex flex-wrap gap-2">
					{#if canRun}
						<Button type="submit" disabled={!workflow.currentVersion}>
							Run version {workflow.currentVersion ?? '—'}
						</Button>
					{/if}
					{#if canWrite}
						<Button
							type="submit"
							name="test"
							value="true"
							variant="secondary"
							disabled={workflow.diagnostics.length > 0}
						>
							Test run the draft
						</Button>
					{/if}
				</div>
				{#if form && 'runMessage' in form && form.runMessage}
					<p class="text-sm text-danger" role="alert">{form.runMessage}</p>
				{/if}
			</form>
		</Card>
	{/if}

	<Card title="Recent runs">
		{#if data.runs.length === 0}
			<p class="text-sm text-fg-muted">No runs yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Recent runs">
				{#each data.runs as run (run.id)}
					<li class="flex items-center justify-between gap-4 py-2 first:pt-0 last:pb-0">
						<a
							href={resolve('/(app)/runs/[id]', { id: run.id })}
							class="text-sm text-fg hover:underline"
						>
							{when(run.createdAt)}
						</a>
						<span class="flex items-center gap-2 text-xs text-fg-muted">
							{run.workflowVersion ? `v${run.workflowVersion}` : 'test'} · {run.triggerType}
							<RunStatusBadge status={run.status} />
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	<div class="grid gap-6 md:grid-cols-2">
		<Card title="Triggers" description="From the published version.">
			{#if data.triggers.length === 0}
				<p class="text-sm text-fg-muted">
					{workflow.currentVersion ? 'Started by hand only.' : 'Publish to activate triggers.'}
				</p>
			{:else}
				<ul class="flex flex-col gap-3">
					{#each data.triggers as trigger (trigger.triggerId)}
						<li class="text-sm">
							<span class="font-medium text-fg">{trigger.triggerId}</span>
							<span class="text-fg-muted">({trigger.type})</span>
							{#if trigger.publicId}
								<p class="mt-1 font-mono text-xs break-all text-fg-muted">
									POST {data.publicUrl}/hooks/{trigger.publicId}
								</p>
							{/if}
							{#if trigger.nextRunAt}
								<p class="mt-1 text-xs text-fg-muted">
									{trigger.type === 'poll' ? 'Next check' : 'Next run'}
									{when(trigger.nextRunAt)}
								</p>
							{/if}
							{#if trigger.lastError}
								<p class="mt-1 text-xs text-danger" role="alert">
									Last check failed: {trigger.lastError}
								</p>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</Card>

		<Card
			title="Runs as"
			description="Runs started by a schedule, webhook or event act as this service account. Runs started by hand act as whoever starts them."
		>
			{#if canAssignRunAs}
				<form method="POST" action="?/runAs" use:enhance={notify} class="flex items-end gap-2">
					<Field label="Service account" for="serviceAccountId">
						<Select
							id="serviceAccountId"
							name="serviceAccountId"
							value={workflow.runAs ?? ''}
							options={runAsOptions}
						/>
					</Field>
					<Button type="submit" variant="secondary">Save</Button>
				</form>
			{:else}
				<p class="text-sm text-fg">{workflow.runAsName ?? 'No identity'}</p>
			{/if}
		</Card>

		{#if canPublish}
			<Card
				title="Save as template"
				description="Offers this draft (and the form that starts it) as a template here and in every organization below."
			>
				<form
					method="POST"
					action="?/saveTemplate"
					use:enhance={notify}
					class="flex flex-col gap-2"
				>
					<Field label="Template name" for="template-name">
						<Input id="template-name" name="name" required maxlength={200} value={workflow.name} />
					</Field>
					<Field label="Category" for="template-category" hint="Default: Your templates">
						<Input id="template-category" name="category" maxlength={100} />
					</Field>
					<Field label="Description" for="template-description">
						<Input id="template-description" name="description" maxlength={2000} />
					</Field>
					<div><Button type="submit" variant="secondary">Save as template</Button></div>
				</form>
			</Card>
		{/if}

		<Card title="Versions">
			{#if data.versions.length > 0}
				<a
					href={resolve('/(app)/workflows/[id]/compare', { id: workflow.id })}
					class="mb-3 inline-block text-sm text-accent hover:underline"
				>
					Compare versions
				</a>
			{/if}
			{#if data.versions.length === 0}
				<p class="text-sm text-fg-muted">Not published yet.</p>
			{:else}
				<ul class="flex flex-col gap-2">
					{#each data.versions as version (version.version)}
						<li class="text-sm">
							<span class="font-medium text-fg">Version {version.version}</span>
							<span class="text-fg-muted"> · {when(version.publishedAt)}</span>
							{#if version.notes}<p class="text-fg-muted">{version.notes}</p>{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</Card>
	</div>
</div>
