<!--
	A text field for an expression, with suggestions while typing inside
	`{{ }}` (an ARIA combobox): arrows move, Enter or Tab accepts, Escape
	closes. The value is committed on change, like the other panel fields.
-->
<script lang="ts">
	import { tick } from 'svelte';
	import { complete, type Completion, type Suggestion } from './suggest.ts';
	import type { PreviewResult } from './context.ts';
	import PreviewButton from './PreviewButton.svelte';

	let {
		id,
		value,
		names,
		readonly = false,
		onchange,
		preview
	}: {
		id: string;
		value: string;
		names: Suggestion[];
		readonly?: boolean;
		onchange: (value: string) => void;
		/** Evaluates the expression against the latest run. */
		preview?: (expression: unknown) => Promise<PreviewResult>;
	} = $props();

	let input = $state<HTMLInputElement>();
	let open = $state<Completion | null>(null);
	let active = $state(0);
	const listId = $derived(`${id}-suggestions`);

	function refresh() {
		if (!input || readonly) return;
		open = complete(input.value, input.selectionStart ?? input.value.length, names);
		active = 0;
	}

	async function accept(item: Suggestion) {
		if (!input || !open) return;
		const text = input.value;
		input.value = text.slice(0, open.from) + item.label + text.slice(open.to);
		const caret = open.from + item.label.length;
		open = null;
		onchange(input.value);
		await tick();
		input.setSelectionRange(caret, caret);
		input.focus();
	}

	function keydown(e: KeyboardEvent) {
		if (!open) return;
		if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
			e.preventDefault();
			const n = open.items.length;
			active = (active + (e.key === 'ArrowDown' ? 1 : n - 1)) % n;
		} else if (e.key === 'Enter' || e.key === 'Tab') {
			e.preventDefault();
			void accept(open.items[active]);
		} else if (e.key === 'Escape') {
			e.preventDefault();
			open = null;
		}
	}
</script>

<div class="relative">
	<input
		bind:this={input}
		{id}
		class="w-full rounded-control border border-border bg-surface px-2 py-1 font-mono text-sm text-fg"
		{value}
		{readonly}
		spellcheck="false"
		autocomplete="off"
		role="combobox"
		aria-autocomplete="list"
		aria-expanded={open !== null}
		aria-controls={listId}
		aria-activedescendant={open ? `${listId}-${active}` : undefined}
		oninput={refresh}
		onclick={refresh}
		onkeydown={keydown}
		onblur={() => (open = null)}
		onchange={(e) => onchange(e.currentTarget.value)}
	/>
	{#if open}
		<ul
			id={listId}
			role="listbox"
			aria-label="Suggestions"
			class="absolute z-20 mt-1 max-h-64 w-full overflow-auto rounded-control border border-border bg-surface py-1 shadow-lg"
		>
			{#each open.items as item, i (item.label)}
				<li
					id={`${listId}-${i}`}
					role="option"
					aria-selected={i === active}
					class="flex cursor-pointer items-baseline justify-between gap-3 px-2 py-1 text-sm {i ===
					active
						? 'bg-surface-sunken'
						: ''}"
					onmousedown={(e) => {
						// Before the input's blur closes the list.
						e.preventDefault();
						void accept(item);
					}}
				>
					<span class="font-mono text-fg">{item.label}</span>
					<span class="truncate text-xs text-fg-muted">{item.detail}</span>
				</li>
			{/each}
		</ul>
	{/if}
	{#if preview && value.includes('{{')}
		<PreviewButton {preview} value={() => input?.value ?? value} />
	{/if}
</div>
