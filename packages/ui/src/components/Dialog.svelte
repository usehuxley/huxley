<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Dialog } from 'bits-ui';
	import { X } from '@lucide/svelte';

	type Props = {
		open?: boolean;
		title: string;
		description?: string;
		/** Footer content, usually action buttons. */
		footer?: Snippet;
		children?: Snippet;
	};

	let { open = $bindable(false), title, description, footer, children }: Props = $props();
</script>

<Dialog.Root bind:open>
	<Dialog.Portal>
		<Dialog.Overlay class="fixed inset-0 z-40 bg-overlay" />
		<Dialog.Content
			class="fixed top-1/2 left-1/2 z-50 w-[calc(100%-2rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 rounded-card border border-border bg-surface-raised shadow-overlay"
		>
			<div class="flex items-start justify-between gap-4 px-5 pt-5">
				<div>
					<Dialog.Title class="text-base font-semibold text-fg">{title}</Dialog.Title>
					{#if description}
						<Dialog.Description class="mt-1 text-sm text-fg-muted">
							{description}
						</Dialog.Description>
					{/if}
				</div>
				<Dialog.Close
					class="rounded-control p-1 text-fg-muted hover:bg-surface-sunken"
					aria-label="Close"
				>
					<X size={18} />
				</Dialog.Close>
			</div>
			{#if children}<div class="px-5 py-4">{@render children()}</div>{/if}
			{#if footer}
				<div class="flex justify-end gap-2 border-t border-border px-5 py-3">
					{@render footer()}
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Portal>
</Dialog.Root>
