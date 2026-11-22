<script lang="ts">
	import { X } from '@lucide/svelte';
	import { toast, type ToastTone } from '../toast.svelte';

	const accents: Record<ToastTone, string> = {
		neutral: 'border-l-border-strong',
		success: 'border-l-success',
		warning: 'border-l-warning',
		danger: 'border-l-danger'
	};
</script>

<!-- Polite live region: screen readers announce toasts without stealing focus. -->
<div
	class="pointer-events-none fixed right-4 bottom-4 z-50 flex w-[calc(100%-2rem)] max-w-sm flex-col gap-2"
	role="status"
	aria-live="polite"
>
	{#each toast.items as item (item.id)}
		<div
			class="pointer-events-auto flex items-start gap-3 rounded-card border border-l-4 border-border bg-surface-raised p-3 shadow-overlay {accents[
				item.tone
			]}"
		>
			<div class="flex-1">
				<p class="text-sm font-medium text-fg">{item.title}</p>
				{#if item.description}<p class="mt-0.5 text-sm text-fg-muted">{item.description}</p>{/if}
			</div>
			<button
				type="button"
				class="rounded-control p-0.5 text-fg-muted hover:bg-surface-sunken"
				aria-label="Dismiss notification"
				onclick={() => toast.dismiss(item.id)}
			>
				<X size={16} />
			</button>
		</div>
	{/each}
</div>
