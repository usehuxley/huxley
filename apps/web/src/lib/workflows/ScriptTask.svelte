<script lang="ts">
	import type { components } from '@usehuxley/api-client';

	type TaskView = components['schemas']['TaskView'];

	let { url, active }: { url: string; active: boolean } = $props();
	let task = $state<TaskView | null>(null);
	let missing = $state(false);

	const done = (t: TaskView | null) => t?.status === 'succeeded' || t?.status === 'failed';

	async function load() {
		const response = await fetch(url).catch(() => null);
		if (response?.ok) {
			task = (await response.json()) as TaskView;
			missing = false;
		} else {
			missing = true;
		}
	}

	$effect(() => {
		void load();
		if (!active) return;
		// While the step is going, check back until the script is done.
		const timer = setInterval(() => {
			if (!done(task)) void load();
		}, 3000);
		return () => clearInterval(timer);
	});

	const summary = $derived.by(() => {
		if (!task) return '';
		const runner = task.runnerName ?? 'a runner';
		switch (task.status) {
			case 'queued':
				return 'Waiting for a runner';
			case 'running':
				return `Running on ${runner}`;
			case 'succeeded':
				return `Ran on ${runner}`;
			case 'failed':
				return task.runnerName ? `Failed on ${runner}` : 'Failed';
		}
	});
</script>

<div class="md:col-span-2" data-testid="script-task">
	<p class="text-xs font-medium text-fg-muted">Script</p>
	{#if task}
		<p class="mt-1 text-xs text-fg-muted">
			{summary} · version {task.scriptVersion}
			{#if task.exitCode !== null && task.exitCode !== undefined}· exit code {task.exitCode}{/if}
		</p>
		{#if task.error}
			<p class="mt-1 text-xs text-danger">{task.error}</p>
		{/if}
		{#if task.logs}
			<pre
				class="mt-1 max-h-96 overflow-auto rounded-control bg-surface-sunken p-2 font-mono text-xs whitespace-pre-wrap text-fg"
				aria-label="Script logs">{task.logs}</pre>
		{:else if done(task)}
			<p class="mt-1 text-xs text-fg-muted">No output.</p>
		{/if}
	{:else if missing}
		<p class="mt-1 text-xs text-fg-muted">Not queued yet.</p>
	{/if}
</div>
