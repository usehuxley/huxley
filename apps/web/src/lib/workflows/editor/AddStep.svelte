<script lang="ts">
	import { Plus } from '@lucide/svelte';
	import { STEPS } from './catalog.ts';

	let { onadd, label = 'Add a step' }: { onadd: (type: string) => void; label?: string } = $props();

	let open = $state(false);
	const groups = ['Actions', 'Data', 'Control flow', 'Finish'] as const;
</script>

<div class="relative flex justify-center py-1">
	<button
		type="button"
		class="flex items-center gap-1 rounded-full border border-dashed border-border px-2 py-0.5 text-xs text-fg-muted hover:border-accent hover:text-accent"
		aria-expanded={open}
		aria-label={label}
		onclick={() => (open = !open)}
	>
		<Plus size={12} />
		<span class="sr-only sm:not-sr-only">Add</span>
	</button>
	{#if open}
		<div
			class="absolute top-full z-20 mt-1 w-72 rounded-control border border-border bg-surface p-2 shadow-lg"
			role="menu"
			aria-label="Step types"
		>
			{#each groups as group (group)}
				<p class="px-2 pt-2 text-xs font-semibold text-fg-muted first:pt-0">{group}</p>
				{#each STEPS.filter((s) => s.group === group) as spec (spec.type)}
					<button
						type="button"
						role="menuitem"
						class="flex w-full flex-col items-start rounded-control px-2 py-1 text-left hover:bg-surface-sunken"
						onclick={() => {
							open = false;
							onadd(spec.type);
						}}
					>
						<span class="text-sm text-fg">{spec.label}</span>
						<span class="text-xs text-fg-muted">{spec.description}</span>
					</button>
				{/each}
			{/each}
		</div>
	{/if}
</div>
