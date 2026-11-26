<script lang="ts">
	import { accentStyle } from '#lib/branding.ts';
	import NpsForm from '#lib/csat/NpsForm.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let n = $derived(data.nps);
</script>

<svelte:head>
	<title>A quick question · {n.branding.name}</title>
	<meta name="robots" content="noindex" />
</svelte:head>

<main
	class="flex min-h-screen items-center justify-center bg-bg px-4"
	style={accentStyle(n.branding.accentColor)}
>
	<div class="w-full max-w-lg rounded-card border border-border bg-surface p-6 shadow-card">
		{#if n.branding.logo}<img src={n.branding.logo} alt="" class="mb-4 h-8" />{/if}
		<h1 class="text-xl font-semibold text-fg">A quick question from {n.company}</h1>
		{#if form && 'answered' in form}
			<p class="mt-4 text-fg" role="status">Thanks for letting us know.</p>
		{:else if n.score !== null && n.score !== undefined}
			<p class="mt-4 text-fg">You answered {n.score} out of 10. Thanks.</p>
		{:else if !n.open}
			<p class="mt-4 text-fg-muted">It’s too late to answer this one. Thanks anyway.</p>
		{:else}
			<div class="mt-5">
				<NpsForm
					action=""
					company={n.company}
					selected={data.selected}
					message={form && 'message' in form ? form.message : null}
				/>
			</div>
		{/if}
	</div>
</main>
