<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let query = $derived(
		form?.query ??
			(data.datasets[0]
				? `SELECT * FROM ds_${data.datasets[0].id.replaceAll('-', '')} LIMIT 20`
				: 'SELECT 1')
	);
	let result = $derived(form && 'result' in form ? form.result : undefined);
	const cell = (v: unknown): string => {
		if (v === null || v === undefined) return '';
		if (typeof v === 'string') return v;
		if (typeof v === 'number' || typeof v === 'boolean') return String(v);
		return JSON.stringify(v);
	};
</script>

<svelte:head>
	<title>SQL console · Datasets · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<header>
		<a href={resolve('/(app)/datasets')} class="text-sm text-fg-muted hover:underline">Datasets</a>
		<h1 class="text-2xl font-semibold text-fg">SQL console</h1>
		<p class="mt-1 text-sm text-fg-muted">
			Read-only queries over every organization's dataset rows. One SELECT at a time, at most 5
			seconds and 1,000 rows. Every query is audited.
		</p>
	</header>
	<Card title="Tables">
		<ul class="flex flex-col gap-1 text-sm" aria-label="Dataset tables">
			{#each data.datasets as dataset (dataset.id)}
				<li>
					<code class="font-mono text-xs">ds_{dataset.id.replaceAll('-', '')}</code>
					<span class="text-fg-muted">{dataset.name} ({dataset.key})</span>
				</li>
			{:else}
				<li class="text-fg-muted">No datasets yet.</li>
			{/each}
		</ul>
		<p class="mt-2 text-xs text-fg-muted">
			Each table has the dataset's columns plus <code>_org_id</code> and <code>_synced_at</code>.
		</p>
	</Card>
	<Card title="Query">
		<form
			method="POST"
			action="?/run"
			use:enhance={() =>
				({ update }) =>
					update({ reset: false })}
			class="flex flex-col gap-3"
		>
			<label for="query" class="sr-only">SQL query</label>
			<textarea
				id="query"
				name="query"
				value={query}
				rows={6}
				spellcheck="false"
				class="rounded-control border border-border bg-surface p-2 font-mono text-sm"></textarea>
			<div><Button type="submit">Run</Button></div>
		</form>
		{#if form && 'message' in form && form.message}
			<p class="mt-3 text-sm text-danger" role="alert">{form.message}</p>
		{/if}
	</Card>
	{#if result}
		<Card
			title="Result"
			description="{result.rows.length} row(s){result.truncated ? ', cut off at 1,000' : ''}."
		>
			<div class="overflow-x-auto">
				<table class="w-full text-left text-sm" aria-label="Query result">
					<thead class="text-xs text-fg-muted">
						<tr>
							{#each result.columns as column, i (i)}<th class="px-2 py-1 font-medium">
									{column}
								</th>{/each}
						</tr>
					</thead>
					<tbody class="divide-y divide-border">
						{#each result.rows as row, r (r)}
							<tr>
								{#each result.columns as column, i (i)}
									<td class="max-w-xs truncate px-2 py-1 font-mono text-xs">{cell(row[column])}</td>
								{/each}
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</Card>
	{/if}
</div>
