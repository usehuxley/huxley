<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import type {
		CodeCompletion,
		CodeDiagnostic,
		CodeLanguage,
		JinjaScope
	} from './code-editor/types';
	import type { Controller } from './code-editor/setup';

	type Props = {
		value?: string;
		language?: CodeLanguage;
		/** Accessible name (required: there's no visible label inside). */
		label: string;
		/** Form field name: the text is submitted with the form. */
		name?: string;
		id?: string;
		readonly?: boolean;
		placeholder?: string;
		/** Height in lines before it scrolls. */
		lines?: number;
		words?: CodeCompletion[];
		jinja?: JinjaScope;
		lint?: (text: string) => CodeDiagnostic[] | Promise<CodeDiagnostic[]>;
		/** Completes with the page's own logic (see `complete` in the setup). */
		complete?: (
			text: string,
			pos: number
		) => { from: number; to?: number; options: CodeCompletion[] } | null;
		/** One or a few lines with no gutters, e.g. an expression field. */
		compact?: boolean;
		/** Called when the person edits it (not when the page sets the value). */
		onchange?: (value: string) => void;
		/** Called when the editor loses focus. */
		onblur?: () => void;
		class?: string;
	};
	let {
		value = $bindable(''),
		language = 'text',
		label,
		name,
		id,
		readonly = false,
		placeholder,
		lines = 16,
		words,
		jinja,
		lint,
		complete,
		compact = false,
		onchange,
		onblur,
		class: className = ''
	}: Props = $props();

	let host: HTMLDivElement;
	let editor: Controller | undefined = $state();

	onMount(() => {
		let cancelled = false;
		let made: Controller | undefined;
		// The editor arrives on demand; until then the textarea works as-is.
		void import('./code-editor/setup').then(async ({ create }) => {
			if (cancelled) return;
			made = await create(host, {
				value: untrack(() => value),
				language: untrack(() => language),
				readonly: untrack(() => readonly),
				label,
				placeholder,
				words: words ? () => words ?? [] : undefined,
				jinja,
				lint,
				complete: complete ? (text, pos) => complete?.(text, pos) ?? null : undefined,
				compact,
				onBlur: () => onblur?.(),
				onChange: (text, byPerson) => {
					value = text;
					if (byPerson) onchange?.(text);
				}
			});
			if (cancelled) {
				made.destroy();
				return;
			}
			// Typed or set while it loaded (the textarea took it): catch up.
			made.setValue(untrack(() => value));
			made.setReadonly(untrack(() => readonly));
			editor = made;
		});
		return () => {
			cancelled = true;
			made?.destroy();
		};
	});

	// Changes from outside (a reset, a loaded version) reach the editor.
	$effect(() => {
		const v = value;
		untrack(() => editor?.setValue(v));
	});
	$effect(() => {
		const r = readonly;
		untrack(() => editor?.setReadonly(r));
	});
</script>

<div
	class="code-editor {className}"
	style="--lines: {lines}"
	{id}
	data-language={language}
	data-compact={compact ? '' : undefined}
>
	{#if editor}
		{#if name}<input type="hidden" {name} {value} />{/if}
	{:else}
		<textarea
			{name}
			aria-label={label}
			rows={lines}
			spellcheck="false"
			{readonly}
			{placeholder}
			class="w-full rounded-control border border-border-strong bg-surface p-3 font-mono text-xs text-fg"
			bind:value></textarea>
	{/if}
	<div bind:this={host} class:hidden={!editor}></div>
</div>

<style>
	/* The scroller, not the editor, carries the height: the editor is a
	   flex column, so limiting it alone lets the gutters spill out. */
	.code-editor :global(.cm-editor) {
		overflow: hidden;
	}
	.code-editor :global(.cm-scroller) {
		max-height: calc(var(--lines) * 1.55em + 1rem);
		min-height: 4.5em;
		overflow: auto;
	}
	.code-editor[data-compact] :global(.cm-scroller) {
		min-height: 0;
	}
	.code-editor[data-compact] :global(.cm-content) {
		padding: 0.25rem 0.25rem;
	}
</style>
