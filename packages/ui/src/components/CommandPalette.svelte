<script lang="ts" module>
	export type Command = {
		id: string;
		label: string;
		/** Extra words to match on, e.g. synonyms. */
		keywords?: string[];
		group?: string;
		run: () => void;
	};
</script>

<script lang="ts">
	import { Dialog } from 'bits-ui';
	import { Search } from '@lucide/svelte';

	type Props = {
		commands: Command[];
		open?: boolean;
		placeholder?: string;
	};

	let {
		commands,
		open = $bindable(false),
		placeholder = 'Search or run a command…'
	}: Props = $props();

	let query = $state('');
	let active = $state(0);

	let results = $derived.by(() => {
		const q = query.trim().toLowerCase();
		if (!q) return commands;
		return commands.filter((c) =>
			[c.label, c.group ?? '', ...(c.keywords ?? [])].some((s) => s.toLowerCase().includes(q))
		);
	});

	$effect(() => {
		// Reset the highlight whenever the result set changes.
		void results;
		active = 0;
	});

	function onGlobalKey(event: KeyboardEvent) {
		if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
			event.preventDefault();
			open = !open;
		}
	}

	function onListKey(event: KeyboardEvent) {
		if (event.key === 'ArrowDown') {
			event.preventDefault();
			active = Math.min(active + 1, results.length - 1);
		} else if (event.key === 'ArrowUp') {
			event.preventDefault();
			active = Math.max(active - 1, 0);
		} else if (event.key === 'Enter' && results[active]) {
			event.preventDefault();
			execute(results[active]);
		}
	}

	function execute(command: Command) {
		open = false;
		query = '';
		command.run();
	}
</script>

<svelte:window onkeydown={onGlobalKey} />

<Dialog.Root bind:open>
	<Dialog.Portal>
		<Dialog.Overlay class="fixed inset-0 z-40 bg-overlay" />
		<Dialog.Content
			class="fixed top-[15vh] left-1/2 z-50 w-[calc(100%-2rem)] max-w-xl -translate-x-1/2 overflow-hidden rounded-card border border-border bg-surface-raised shadow-overlay"
		>
			<Dialog.Title class="sr-only">Command palette</Dialog.Title>
			<div class="flex items-center gap-2 border-b border-border px-4">
				<Search size={18} class="text-fg-subtle" />
				<input
					bind:value={query}
					onkeydown={onListKey}
					{placeholder}
					class="h-12 flex-1 bg-transparent text-sm text-fg outline-none placeholder:text-fg-subtle"
					role="combobox"
					aria-expanded="true"
					aria-controls="hx-command-list"
					aria-activedescendant={results[active] ? `hx-cmd-${results[active].id}` : undefined}
				/>
			</div>
			<ul id="hx-command-list" role="listbox" class="max-h-80 overflow-y-auto p-2">
				{#each results as command, i (command.id)}
					<li
						id="hx-cmd-{command.id}"
						role="option"
						aria-selected={i === active}
						class="flex cursor-pointer items-center justify-between rounded-control px-3 py-2 text-sm {i ===
						active
							? 'bg-accent-soft text-fg'
							: 'text-fg-muted'}"
						onclick={() => execute(command)}
						onkeydown={() => {}}
						onmousemove={() => (active = i)}
					>
						<span>{command.label}</span>
						{#if command.group}<span class="text-xs text-fg-subtle">{command.group}</span>{/if}
					</li>
				{:else}
					<li class="px-3 py-6 text-center text-sm text-fg-muted">No matches.</li>
				{/each}
			</ul>
		</Dialog.Content>
	</Dialog.Portal>
</Dialog.Root>
