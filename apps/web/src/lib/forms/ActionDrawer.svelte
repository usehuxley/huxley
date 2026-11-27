<script lang="ts">
	import { deserialize } from '$app/forms';
	import { invalidateAll, preloadData } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Check, Play, Zap } from '@lucide/svelte';
	import {
		Button,
		Drawer,
		FormRenderer,
		answersOf,
		fieldsOf,
		toast,
		tray,
		type FormField
	} from '@usehuxley/ui';
	import RunStatusBadge from '#lib/workflows/RunStatusBadge.svelte';
	import type { ActionForm } from './actions.server.ts';

	/**
	 * Runs an Action (a form that starts workflows) beside the page: fill it in,
	 * run it, and watch its steps without leaving the record. `href` is the
	 * form's page with the record in the query, which also does the submitting.
	 */
	let {
		open = $bindable(false),
		action,
		href
	}: { open?: boolean; action: ActionForm; href: string } = $props();

	type Loaded = {
		form: {
			id: string;
			name: string;
			description?: string | null;
			schema: unknown;
			ui: unknown;
			archived?: boolean;
		};
		on: {
			orgId: string;
			orgName: string;
			contactId: string;
			contactName: string;
			ticketId: string;
			ticketReference: string;
			ticketTitle: string;
		} | null;
	};
	type Step = { stepKey: string; stepType: string; status: string };
	type Run = { id: string; status: string; steps: Step[] };

	let loaded = $state<Loaded | null>(null);
	let failed = $state('');
	let values = $state<Record<string, unknown>>({});
	let errors = $state<Record<string, string>>({});
	let submitting = $state(false);
	let runs = $state<Run[]>([]);
	let skipped = $state<string[]>([]);
	let submitted = $state(false);

	let fields: FormField[] = $derived(
		loaded ? fieldsOf(loaded.form.schema as never, loaded.form.ui as never) : []
	);
	let done = $derived(
		submitted && runs.every((r) => !['queued', 'running', 'waiting'].includes(r.status))
	);
	let on = $derived(loaded?.on ?? null);
	const optionsUrl = (name: string) =>
		`${resolve('/(app)/forms/[id]/fields/[name]/options', { id: action.id, name })}?orgId=${encodeURIComponent(on?.orgId ?? '')}`;

	// Load the form when the panel opens for an Action; start fresh each time.
	$effect(() => {
		if (!open) return;
		const target = href;
		loaded = null;
		failed = '';
		values = {};
		errors = {};
		runs = [];
		skipped = [];
		submitted = false;
		void preloadData(target).then((result) => {
			if (result.type === 'loaded' && result.status === 200) {
				loaded = result.data as unknown as Loaded;
			} else {
				failed = 'Could not load this Action.';
			}
		});
	});

	function follow(id: string) {
		const source = new EventSource(resolve('/(app)/runs/[id]/stream', { id }));
		source.addEventListener('run', (e) => {
			const next = JSON.parse((e as MessageEvent<string>).data) as Run;
			runs = runs.map((r) => (r.id === id ? next : r));
			const active = ['queued', 'running', 'waiting'].includes(next.status);
			tray.update(`run:${id}`, {
				state: active ? 'running' : next.status === 'failed' ? 'failed' : 'done',
				subtitle: next.status
			});
			if (!active) {
				source.close();
				void invalidateAll();
				toast.push({
					title:
						next.status === 'succeeded'
							? `${action.name} finished.`
							: `${action.name}: ${next.status}.`,
					tone: next.status === 'succeeded' ? 'success' : 'danger'
				});
			}
		});
		source.onerror = () => source.close();
	}

	async function run(event: SubmitEvent) {
		event.preventDefault();
		if (!loaded) return;
		submitting = true;
		errors = {};
		const body = new FormData();
		body.set('orgId', on?.orgId ?? '');
		body.set('contactId', on?.contactId ?? '');
		body.set('ticketId', on?.ticketId ?? '');
		body.set('answers', JSON.stringify(answersOf(fields, values)));
		try {
			const response = await fetch(`${href}${href.includes('?') ? '&' : '?'}/submit`, {
				method: 'POST',
				body,
				headers: { 'x-sveltekit-action': 'true' }
			});
			const result = deserialize(await response.text());
			if (result.type === 'success') {
				const data = result.data as { submitted: { runs: string[]; skipped: string[] } };
				runs = data.submitted.runs.map((id) => ({ id, status: 'queued', steps: [] }));
				skipped = data.submitted.skipped;
				submitted = true;
				for (const id of data.submitted.runs) {
					tray.open({
						key: `run:${id}`,
						kind: 'run',
						href: `/runs/${id}`,
						title: `Run: ${action.name}`,
						subtitle: 'queued',
						state: 'running'
					});
					follow(id);
				}
				if (runs.length === 0) void invalidateAll();
			} else if (result.type === 'failure') {
				const data = result.data as { message?: string; errors?: Record<string, string> };
				errors = data?.errors ?? {};
				failed = data?.message ?? 'Could not run it.';
			} else {
				failed = 'Could not run it.';
			}
		} catch {
			failed = 'Could not reach Huxley. Try again.';
		} finally {
			submitting = false;
		}
	}
</script>

<Drawer
	bind:open
	title={action.name}
	description={on
		? `On ${[on.ticketReference, on.contactName, on.orgName].filter(Boolean).join(' · ')}`
		: undefined}
>
	{#snippet lead()}
		<span class="grid size-8 flex-none place-items-center rounded-md bg-accent-soft text-accent">
			<Zap size={16} />
		</span>
	{/snippet}

	{#if !loaded && !failed}
		<p class="text-sm text-fg-muted">Loading…</p>
	{:else if submitted}
		<div class="flex flex-col gap-3" role="status">
			{#if runs.length === 0}
				<p class="text-sm text-fg">Saved. No workflows are listening for this Action yet.</p>
			{/if}
			{#each runs as r (r.id)}
				<div class="flex flex-col gap-2 rounded-card border border-border p-3">
					<div class="flex items-center justify-between gap-2">
						<a
							href={resolve('/(app)/runs/[id]', { id: r.id })}
							class="font-mono text-sm text-accent hover:underline"
						>
							Run {r.id.slice(0, 8)}
						</a>
						<RunStatusBadge status={r.status} />
					</div>
					{#if r.steps.length > 0}
						<ol class="flex flex-col gap-1" aria-label="Steps of run {r.id.slice(0, 8)}">
							{#each r.steps as step (step.stepKey)}
								<li
									class="flex items-center gap-2 rounded-control px-2 py-1.5 text-sm {step.status ===
									'running'
										? 'bg-accent-soft text-fg'
										: 'text-fg-muted'}"
								>
									<span
										class="grid size-5 flex-none place-items-center rounded-full border-2 {step.status ===
										'succeeded'
											? 'border-success bg-success text-white'
											: step.status === 'running'
												? 'animate-spin border-accent border-t-transparent'
												: step.status === 'failed'
													? 'border-danger bg-danger text-white'
													: 'border-border-strong'}"
										aria-hidden="true"
									>
										{#if step.status === 'succeeded'}<Check size={11} strokeWidth={3} />{/if}
									</span>
									<span class="font-mono">{step.stepKey}</span>
									<span class="ml-auto text-xs">{step.status}</span>
								</li>
							{/each}
						</ol>
					{:else if !done}
						<p class="text-sm text-fg-muted">Starting…</p>
					{/if}
				</div>
			{/each}
			{#if skipped.length > 0}
				<p class="text-sm text-danger">
					Not started (their inputs don't match this form): {skipped.join(', ')}
				</p>
			{/if}
		</div>
	{:else if loaded}
		<form id="action-form" class="flex flex-col gap-4" onsubmit={run}>
			{#if loaded.form.description}
				<p class="text-sm text-fg-muted">{loaded.form.description}</p>
			{/if}
			<FormRenderer
				schema={loaded.form.schema as never}
				ui={loaded.form.ui as never}
				bind:values
				{errors}
				{optionsUrl}
			/>
		</form>
	{/if}
	{#if failed}
		<p class="text-sm text-danger" role="alert">{failed}</p>
	{/if}

	{#snippet footer()}
		<span class="text-xs text-fg-subtle">
			{submitted
				? done
					? 'Finished.'
					: 'Running. You can close this; it stays in your tray.'
				: 'You stay on this page while it runs.'}
		</span>
		<span class="flex-1"></span>
		{#if submitted}
			<Button variant={done ? 'primary' : 'secondary'} onclick={() => (open = false)}>
				{done ? 'Done' : 'Close'}
			</Button>
		{:else}
			<Button
				type="submit"
				form="action-form"
				variant="primary"
				disabled={!loaded || submitting || loaded.form.archived}
			>
				<Play size={14} /> Run
			</Button>
		{/if}
	{/snippet}
</Drawer>
