<script lang="ts">
	import { X } from '@lucide/svelte';
	import { STEPS } from './catalog.ts';
	import { stepIcon } from './icons.ts';

	let {
		where,
		onpick,
		oncancel
	}: {
		/** Where the step goes, e.g. "Add a step after greet". */
		where: string;
		onpick: (type: string) => void;
		oncancel: () => void;
	} = $props();

	const groups = ['Actions', 'Data', 'Control flow', 'Finish'] as const;
	let menu = $state<HTMLElement>();
	$effect(() => {
		menu?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
	});
</script>

<div class="flex flex-col gap-2">
	<div class="flex items-start justify-between gap-2">
		<div>
			<p class="text-xs font-medium tracking-wide text-fg-muted uppercase">Add a step</p>
			<p class="text-sm text-fg">{where.replace(/^Add a step /, 'Goes ')}</p>
		</div>
		<button
			type="button"
			class="rounded-control p-1 text-fg-muted hover:bg-surface-sunken hover:text-fg"
			aria-label="Cancel adding a step"
			onclick={oncancel}
		>
			<X size={16} />
		</button>
	</div>
	<div
		bind:this={menu}
		role="menu"
		aria-label="Step types"
		tabindex="-1"
		class="flex max-h-[60vh] flex-col overflow-y-auto"
		onkeydown={(e) => {
			if (e.key === 'Escape') oncancel();
			if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
			e.preventDefault();
			const items = [...(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
			const at = items.indexOf(document.activeElement as HTMLElement);
			items[(at + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length]?.focus();
		}}
	>
		{#each groups as group (group)}
			<p class="px-2 pt-3 text-xs font-semibold text-fg-muted first:pt-0">{group}</p>
			{#each STEPS.filter((s) => s.group === group) as spec (spec.type)}
				{@const Icon = stepIcon(spec.group, spec.type)}
				<button
					type="button"
					role="menuitem"
					class="flex w-full items-start gap-2 rounded-control px-2 py-1.5 text-left hover:bg-surface-sunken focus:bg-surface-sunken focus:outline-none"
					onclick={() => onpick(spec.type)}
				>
					<Icon size={16} class="mt-0.5 shrink-0 text-fg-muted" />
					<span>
						<span class="block text-sm text-fg">{spec.label}</span>
						<span class="block text-xs text-fg-muted">{spec.description}</span>
					</span>
				</button>
			{/each}
		{/each}
	</div>
</div>
