<script lang="ts">
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Button, Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let options = $derived([
		{ value: 'draft', label: 'Draft' },
		...data.versions.map((v) => ({ value: String(v.version), label: `Version ${v.version}` }))
	]);
	let changes = $derived(data.diff?.filter((l) => l.kind !== ' ').length ?? 0);
	const label = (which: string) => (which === 'draft' ? 'the draft' : `version ${which}`);
</script>

<svelte:head>
	<title>Compare · {data.workflow.name} · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<a
		href={resolve('/(app)/workflows/[id]', { id: data.workflow.id })}
		class="flex w-fit items-center gap-1 text-sm text-fg-muted hover:text-fg"
	>
		<ArrowLeft size={16} />
		{data.workflow.name}
	</a>
	<h1 class="text-2xl font-semibold text-fg">Compare versions</h1>

	<form method="GET" class="flex flex-wrap items-end gap-3">
		<label class="flex flex-col gap-1 text-sm text-fg">
			From
			<select
				name="from"
				value={data.from}
				class="rounded-control border border-border bg-surface px-2 py-1 text-sm"
			>
				{#each options as option (option.value)}<option value={option.value}>
						{option.label}
					</option>{/each}
			</select>
		</label>
		<label class="flex flex-col gap-1 text-sm text-fg">
			To
			<select
				name="to"
				value={data.to}
				class="rounded-control border border-border bg-surface px-2 py-1 text-sm"
			>
				{#each options as option (option.value)}<option value={option.value}>
						{option.label}
					</option>{/each}
			</select>
		</label>
		<Button type="submit" variant="secondary">Compare</Button>
	</form>

	<Card
		title={`From ${label(data.from)} to ${label(data.to)}`}
		description={data.diff
			? changes === 0
				? 'No differences.'
				: `${changes} line${changes === 1 ? '' : 's'} changed.`
			: undefined}
	>
		{#if !data.diff}
			<p class="text-sm text-fg-muted">Too long to compare line by line.</p>
		{:else}
			<pre
				class="overflow-x-auto font-mono text-xs leading-5"
				aria-label="Differences">{#each data.diff as line, i (i)}<span
						class="block {line.kind === '+'
							? 'bg-success/10 text-success'
							: line.kind === '-'
								? 'bg-danger/10 text-danger'
								: 'text-fg-muted'}">{line.kind} {line.text}</span
					>{/each}</pre>
		{/if}
	</Card>
</div>
