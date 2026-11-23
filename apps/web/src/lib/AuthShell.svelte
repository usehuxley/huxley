<script lang="ts">
	import type { Snippet } from 'svelte';

	let { title, children }: { title: string; children: Snippet } = $props();
</script>

<svelte:head>
	<title>{title} · Huxley</title>
</svelte:head>

<main class="flex min-h-screen items-center justify-center bg-bg px-4">
	<div class="w-full max-w-sm">
		<div class="mb-8 flex flex-col items-center gap-3 text-center">
			<span
				class="flex size-11 items-center justify-center rounded-card bg-accent text-xl font-bold text-accent-fg"
				aria-hidden="true"
			>
				H
			</span>
			<h1 class="text-xl font-semibold text-fg">{title}</h1>
		</div>
		<div class="rounded-card border border-border bg-surface p-6 shadow-card">
			{@render children()}
		</div>
	</div>
</main>
