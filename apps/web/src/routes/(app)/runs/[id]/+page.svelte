<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Button, Card } from '@usehuxley/ui';
	import RunStatusBadge from '#lib/workflows/RunStatusBadge.svelte';
	import ScriptTask from '#lib/workflows/ScriptTask.svelte';
	import type { PageProps } from './$types';

	type Run = PageProps['data']['run'];

	let { data, form }: PageProps = $props();
	// The latest pushed state, for the loaded run it was pushed for: a fresh
	// load (after resuming, say) takes over until the stream catches up.
	// Raw: a deep proxy would break the identity check below.
	let live = $state.raw<{ base: Run; run: Run } | null>(null);
	let run = $derived(live?.base === data.run ? live.run : data.run);
	let active = $derived(run.status === 'queued' || run.status === 'running');
	let canRerunFrom = $derived(data.user.grantedPermissions.includes('workflow:write'));
	let canCancel = $derived(
		data.user.grantedPermissions.includes(run.workflowVersion ? 'workflow:run' : 'workflow:write')
	);

	const isActive = (r: Run) => r.status === 'queued' || r.status === 'running';

	// Follow a run in progress: the server pushes each change.
	$effect(() => {
		const base = data.run;
		if (!isActive(base)) return;
		const source = new EventSource(resolve('/(app)/runs/[id]/stream', { id: base.id }));
		source.addEventListener('run', (e) => {
			const next = JSON.parse((e as MessageEvent<string>).data) as Run;
			live = { base, run: next };
			if (!isActive(next)) source.close();
		});
		return () => source.close();
	});

	function json(value: unknown): string {
		return JSON.stringify(value, null, 2);
	}
	/** Size of a value kept in object storage (its row holds a marker), or null. */
	function spilled(value: unknown): number | null {
		if (typeof value !== 'object' || value === null || !('$spilled' in value)) return null;
		const marker = value.$spilled;
		if (typeof marker !== 'object' || marker === null || !('bytes' in marker)) return null;
		const bytes = marker.bytes;
		return typeof bytes === 'number' ? bytes : null;
	}
	function size(bytes: number): string {
		return bytes < 1024 * 1024
			? `${Math.round(bytes / 1024)} KB`
			: `${(bytes / 1024 / 1024).toFixed(1)} MB`;
	}
	function when(value: string | null | undefined): string {
		return value ? new Date(value).toLocaleString() : '—';
	}
	function took(start: string, end: string | null | undefined): string {
		if (!end) return '';
		const ms = new Date(end).getTime() - new Date(start).getTime();
		return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;
	}
</script>

<svelte:head>
	<title>Run · {data.workflow?.name ?? 'Workflow'} · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	{#if data.workflow}
		<a
			href={resolve('/(app)/workflows/[id]', { id: data.workflow.id })}
			class="flex w-fit items-center gap-1 text-sm text-fg-muted hover:text-fg"
		>
			<ArrowLeft size={16} />
			{data.workflow.name}
		</a>
	{/if}

	<header class="flex flex-wrap items-start justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-fg">Run</h1>
			<p class="mt-1 text-sm text-fg-muted">
				{run.workflowVersion ? `Version ${run.workflowVersion}` : 'Test run of the draft'} · started by
				{run.triggerType} · {when(run.createdAt)}
			</p>
		</div>
		<div class="flex items-center gap-2" aria-live="polite">
			<RunStatusBadge status={run.status} />
			{#if run.status === 'failed' && canCancel}
				<form method="POST" action="?/resume" use:enhance>
					<Button type="submit" size="sm">Resume</Button>
				</form>
			{/if}
			{#if active && canCancel && !run.cancelRequested}
				<form method="POST" action="?/cancel" use:enhance>
					<Button type="submit" variant="secondary" size="sm">Cancel</Button>
				</form>
			{/if}
		</div>
	</header>
	{#if form && 'message' in form && form.message}
		<p class="text-sm text-fg-muted" role="status">{form.message}</p>
	{/if}

	{#if run.error}
		<Card title="Error">
			<p class="text-sm text-danger">
				{typeof run.error.message === 'string' ? run.error.message : 'The run failed.'}
			</p>
			<pre class="mt-2 overflow-x-auto text-xs text-fg-muted">{json(run.error)}</pre>
		</Card>
	{/if}

	<Card title="Steps" description="In the order they started; loop iterations show their index.">
		{#if run.steps.length === 0}
			<p class="text-sm text-fg-muted">{active ? 'Starting…' : 'No steps ran.'}</p>
		{:else}
			<ol class="flex flex-col divide-y divide-border" aria-label="Steps">
				{#each run.steps as step (step.stepKey)}
					<li class="py-2 first:pt-0 last:pb-0">
						<details>
							<summary class="flex cursor-pointer items-center justify-between gap-4">
								<span class="min-w-0">
									<span class="font-mono text-sm text-fg">{step.stepKey}</span>
									<span class="text-xs text-fg-muted"> {step.stepType}</span>
									{#if step.attempt > 1}<span class="text-xs text-fg-muted">
											· attempt {step.attempt}
										</span>{/if}
								</span>
								<span class="flex items-center gap-2 text-xs text-fg-muted">
									{took(step.startedAt, step.finishedAt)}
									<RunStatusBadge status={step.status} />
								</span>
							</summary>
							{#if canRerunFrom && data.workflow && data.topLevel.indexOf(step.stepKey) > 0}
								<form method="POST" action="?/rerunFrom" use:enhance class="mt-2">
									<input type="hidden" name="workflowId" value={data.workflow.id} />
									<input type="hidden" name="stepId" value={step.stepKey} />
									<Button type="submit" size="sm" variant="secondary">Run again from here</Button>
									<span class="ml-2 text-xs text-fg-muted">
										A test run; the steps before keep this run's results.
									</span>
								</form>
							{/if}
							<div class="mt-2 grid gap-2 md:grid-cols-2">
								{#if spilled(step.input) !== null}
									<div>
										<p class="text-xs font-medium text-fg-muted">Input</p>
										<p class="mt-1 text-xs text-fg-muted">
											Too large to show here ({size(spilled(step.input) ?? 0)}).
											<a
												class="text-accent underline"
												href={resolve('/(app)/runs/[id]/steps/[key]/[attempt]/[field]', {
													id: run.id,
													key: step.stepKey,
													attempt: String(step.attempt),
													field: 'input'
												})}
												download
											>
												Download JSON
											</a>
										</p>
									</div>
								{:else if step.input !== null && step.input !== undefined}
									<div>
										<p class="text-xs font-medium text-fg-muted">Input</p>
										<pre
											class="mt-1 max-h-80 overflow-auto rounded-control bg-surface-sunken p-2 text-xs text-fg">{json(
												step.input
											)}</pre>
									</div>
								{/if}
								{#if step.error}
									<div>
										<p class="text-xs font-medium text-danger">Error</p>
										<pre
											class="mt-1 max-h-80 overflow-auto rounded-control bg-surface-sunken p-2 text-xs text-fg">{json(
												step.error
											)}</pre>
									</div>
								{:else if spilled(step.output) !== null}
									<div>
										<p class="text-xs font-medium text-fg-muted">Output</p>
										<p class="mt-1 text-xs text-fg-muted">
											Too large to show here ({size(spilled(step.output) ?? 0)}).
											<a
												class="text-accent underline"
												href={resolve('/(app)/runs/[id]/steps/[key]/[attempt]/[field]', {
													id: run.id,
													key: step.stepKey,
													attempt: String(step.attempt),
													field: 'output'
												})}
												download
											>
												Download JSON
											</a>
										</p>
									</div>
								{:else if step.output !== null && step.output !== undefined}
									<div>
										<p class="text-xs font-medium text-fg-muted">Output</p>
										<pre
											class="mt-1 max-h-80 overflow-auto rounded-control bg-surface-sunken p-2 text-xs text-fg">{json(
												step.output
											)}</pre>
									</div>
								{/if}
								{#if step.stepType === 'script'}
									<ScriptTask
										url={resolve('/(app)/runs/[id]/steps/[key]/[attempt]/script', {
											id: run.id,
											key: step.stepKey,
											attempt: String(step.attempt)
										})}
										active={step.status === 'running' || step.status === 'waiting'}
									/>
								{/if}
							</div>
						</details>
					</li>
				{/each}
			</ol>
		{/if}
	</Card>

	<div class="grid gap-6 md:grid-cols-2">
		<Card title="Inputs">
			<pre class="overflow-x-auto text-xs text-fg">{json(run.inputs)}</pre>
		</Card>
		<Card title="Outputs">
			{#if run.outputs}
				<pre class="overflow-x-auto text-xs text-fg" aria-label="Run outputs">{json(
						run.outputs
					)}</pre>
			{:else}
				<p class="text-sm text-fg-muted">{active ? 'Not finished yet.' : 'None.'}</p>
			{/if}
		</Card>
	</div>
</div>
