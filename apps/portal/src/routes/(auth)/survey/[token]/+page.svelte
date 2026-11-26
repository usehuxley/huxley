<script lang="ts">
	import { accentStyle } from '#lib/branding.ts';
	import RatingForm from '#lib/csat/RatingForm.svelte';
	import { ratingText } from '#lib/csat/choices.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let s = $derived(data.survey);
</script>

<svelte:head>
	<title>How did we do? · {s.branding.name}</title>
	<meta name="robots" content="noindex" />
</svelte:head>

<main
	class="flex min-h-screen items-center justify-center bg-bg px-4"
	style={accentStyle(s.branding.accentColor)}
>
	<div class="w-full max-w-md rounded-card border border-border bg-surface p-6 shadow-card">
		{#if s.branding.logo}<img src={s.branding.logo} alt="" class="mb-4 h-8" />{/if}
		<p class="text-sm text-fg-muted">{s.branding.name} · {s.reference}</p>
		<h1 class="mt-1 text-xl font-semibold text-fg">{s.title}</h1>
		{#if form && 'answered' in form}
			<p class="mt-4 text-fg" role="status">Thanks for letting us know.</p>
		{:else if s.rating}
			<p class="mt-4 text-fg">You answered: {ratingText(s.scale, s.rating)}. Thanks.</p>
			{#if s.comment}<p class="mt-2 text-sm text-fg-muted">“{s.comment}”</p>{/if}
		{:else if !s.open}
			<p class="mt-4 text-fg-muted">It’s too late to answer this one. Thanks anyway.</p>
		{:else}
			<div class="mt-5">
				<RatingForm
					scale={s.scale}
					action=""
					selected={data.selected}
					message={form && 'message' in form ? form.message : null}
				/>
			</div>
		{/if}
	</div>
</main>
