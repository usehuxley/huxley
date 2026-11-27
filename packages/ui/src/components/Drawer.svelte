<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Dialog } from 'bits-ui';
	import { X } from '@lucide/svelte';

	/** A panel that slides over from the right, for doing something without leaving the page. */
	type Props = {
		open?: boolean;
		title: string;
		description?: string;
		/** Before the title, e.g. an icon. */
		lead?: Snippet;
		/** Footer content, usually action buttons. */
		footer?: Snippet;
		children?: Snippet;
	};

	let { open = $bindable(false), title, description, lead, footer, children }: Props = $props();
</script>

<Dialog.Root bind:open>
	<Dialog.Portal>
		<Dialog.Overlay class="fixed inset-0 z-40 bg-overlay" />
		<Dialog.Content
			class="drawer fixed inset-y-0 right-0 z-50 flex w-full max-w-md flex-col border-l border-border bg-surface-raised pt-[env(safe-area-inset-top,0px)] pr-[env(safe-area-inset-right,0px)] pb-[env(safe-area-inset-bottom,0px)] shadow-overlay"
		>
			<div class="flex shrink-0 items-start gap-3 border-b border-border px-5 py-4">
				{#if lead}{@render lead()}{/if}
				<div class="min-w-0 flex-1">
					<Dialog.Title class="text-lg leading-tight font-extrabold text-fg">{title}</Dialog.Title>
					{#if description}
						<Dialog.Description class="mt-1 text-[12.5px] text-fg-subtle">
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
			{#if children}
				<div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 py-4">
					{@render children()}
				</div>
			{/if}
			{#if footer}
				<div class="flex shrink-0 items-center gap-2 border-t border-border px-5 py-3">
					{@render footer()}
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Portal>
</Dialog.Root>

<style>
	:global(.drawer) {
		animation: drawer-in 0.22s cubic-bezier(0.2, 0.8, 0.2, 1);
	}
	@keyframes drawer-in {
		from {
			transform: translateX(24px);
			opacity: 0;
		}
	}
</style>
