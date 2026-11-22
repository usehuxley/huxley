<script lang="ts">
	import type { Snippet } from 'svelte';

	type Props = {
		title: string;
		description?: string;
		icon?: Snippet;
		/** Call-to-action buttons. Every empty state should offer a next step. */
		actions?: Snippet;
	};

	let { title, description, icon, actions }: Props = $props();
</script>

<div class="flex flex-col items-center px-6 py-12 text-center">
	{#if icon}
		<div class="mb-4 rounded-full bg-accent-soft p-3 text-accent">{@render icon()}</div>
	{/if}
	<h3 class="text-base font-semibold text-fg">{title}</h3>
	{#if description}<p class="mt-1 max-w-md text-sm text-fg-muted">{description}</p>{/if}
	{#if actions}<div class="mt-5 flex flex-wrap justify-center gap-2">{@render actions()}</div>{/if}
</div>
