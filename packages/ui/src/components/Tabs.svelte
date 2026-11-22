<script lang="ts" module>
	export type TabItem = { value: string; label: string; disabled?: boolean };
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Tabs } from 'bits-ui';

	type Props = {
		items: TabItem[];
		value?: string;
		/** Renders the panel for the active tab. */
		panel: Snippet<[string]>;
		label?: string;
	};

	let { items, value = $bindable(items[0]?.value ?? ''), panel, label }: Props = $props();
</script>

<Tabs.Root bind:value>
	<Tabs.List class="flex gap-1 border-b border-border" aria-label={label}>
		{#each items as item (item.value)}
			<Tabs.Trigger
				value={item.value}
				disabled={item.disabled}
				class="-mb-px border-b-2 border-transparent px-3 py-2 text-sm font-medium text-fg-muted hover:text-fg data-[state=active]:border-accent data-[state=active]:text-fg"
			>
				{item.label}
			</Tabs.Trigger>
		{/each}
	</Tabs.List>
	{#each items as item (item.value)}
		<Tabs.Content value={item.value} class="pt-4">{@render panel(item.value)}</Tabs.Content>
	{/each}
</Tabs.Root>
