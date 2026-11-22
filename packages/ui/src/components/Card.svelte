<script lang="ts">
	import type { Snippet } from 'svelte';

	type Props = {
		title?: string;
		description?: string;
		/** Rendered at the right of the header, e.g. buttons. */
		actions?: Snippet;
		class?: string;
		children: Snippet;
	};

	let { title, description, actions, class: className = '', children }: Props = $props();
</script>

<section class="rounded-card border border-border bg-surface shadow-card {className}">
	{#if title || actions}
		<header class="flex items-start justify-between gap-4 border-b border-border px-5 py-4">
			<div>
				{#if title}<h2 class="text-base font-semibold text-fg">{title}</h2>{/if}
				{#if description}<p class="mt-0.5 text-sm text-fg-muted">{description}</p>{/if}
			</div>
			{#if actions}<div class="flex shrink-0 gap-2">{@render actions()}</div>{/if}
		</header>
	{/if}
	<div class="px-5 py-4">{@render children()}</div>
</section>
