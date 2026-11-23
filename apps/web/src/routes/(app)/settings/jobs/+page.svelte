<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>Workflow jobs · Settings · Huxley</title>
</svelte:head>

<Card
	title="Jobs set aside"
	description="Workflow jobs that kept failing for reasons outside the workflow (the database or a worker crashing, say). A failed step's job has already failed its run: fix the cause and resume the run instead."
>
	{#if form?.message}<p class="mb-3 text-sm text-fg-muted" role="status">{form.message}</p>{/if}
	{#if data.jobs.length === 0}
		<p class="text-sm text-fg-muted">None. Every job either finished or is still in the queue.</p>
	{:else}
		<ul class="flex flex-col divide-y divide-border" aria-label="Jobs set aside">
			{#each data.jobs as job (job.id)}
				<li class="flex flex-wrap items-start justify-between gap-3 py-3 first:pt-0 last:pb-0">
					<div class="min-w-0">
						<p class="text-sm font-medium text-fg">
							{job.kind}{job.stepKey ? ` · ${job.stepKey}` : ''}
							<span class="font-normal text-fg-muted">
								· {job.attempts} attempts · {new Date(job.createdAt).toLocaleString()}
							</span>
						</p>
						{#if job.lastError}<p class="mt-1 font-mono text-xs break-all text-danger">
								{job.lastError}
							</p>{/if}
						{#if job.runId}
							<a
								href={resolve('/(app)/runs/[id]', { id: job.runId })}
								class="text-sm text-accent hover:underline"
							>
								Open the run
							</a>
						{/if}
					</div>
					{#if job.kind !== 'step'}
						<form method="POST" action="?/retry" use:enhance>
							<input type="hidden" name="id" value={job.id} />
							<Button type="submit" variant="secondary" size="sm">Retry</Button>
						</form>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</Card>
