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
		class: className = ''
	}: Props = $props();

	let host: HTMLDivElement;

	/** The page's nonce for style elements (a meta tag the server fills in;
	 * an unfilled placeholder means there's none). */
	function styleNonce(): string | undefined {
		const value = document.querySelector<HTMLMetaElement>(
			'meta[name="huxley-style-nonce"]'
		)?.content;
		return value && !value.startsWith('%') ? value : undefined;
	}
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
				cspNonce: styleNonce(),
				onChange: (text) => (value = text)
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

<div class="code-editor {className}" style="--lines: {lines}" {id} data-language={language}>
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
	<!-- Never display:none: CodeMirror measures itself (and decides whether
	     it's on screen, for its tooltips) as it's created. -->
	<div bind:this={host}></div>
</div>

<style>
	/* The scroller, not the editor, carries the height: the editor is a
	   flex column, so limiting it alone lets the gutters spill out. (No
	   overflow clipping on the editor: CodeMirror reads that as the cursor
	   being out of view and hides its tooltips.) */
	.code-editor :global(.cm-scroller) {
		max-height: calc(var(--lines) * 1.55em + 1rem);
		min-height: 4.5em;
		overflow: auto;
	}
</style>
