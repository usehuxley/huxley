<script lang="ts">
	import type { Snippet } from 'svelte';

	type Props = {
		label: string;
		/** id of the control inside, so the label is associated with it. */
		for: string;
		hint?: string;
		error?: string;
		required?: boolean;
		/** Extra classes for the wrapper, e.g. to span grid columns. */
		class?: string;
		children: Snippet;
	};

	let {
		label,
		for: htmlFor,
		hint,
		error,
		required = false,
		class: className = '',
		children
	}: Props = $props();
</script>

<div class="flex flex-col gap-1.5 {className}">
	<label for={htmlFor} class="text-sm font-medium text-fg">
		{label}
		{#if required}<span class="text-danger" aria-hidden="true">*</span>{/if}
	</label>
	{@render children()}
	{#if error}
		<p id="{htmlFor}-error" class="text-sm text-danger">{error}</p>
	{:else if hint}
		<p id="{htmlFor}-hint" class="text-sm text-fg-muted">{hint}</p>
	{/if}
</div>
