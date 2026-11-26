<!--
	An expression field: the code editor in Jinja, compact. While typing
	inside `{{ }}` it completes what the step can read (inputs, earlier
	steps, loop items, run variables, filters and functions); problems the
	server finds in it are underlined as you type. Edits are committed
	shortly after typing stops, and when the field goes away.
-->
<script lang="ts">
	import { onDestroy, untrack } from 'svelte';
	import { CodeEditor, type CodeDiagnostic } from '@usehuxley/ui';
	import { complete, type Suggestion } from './suggest.ts';
	import type { PreviewResult } from './context.ts';
	import PreviewButton from './PreviewButton.svelte';

	let {
		id,
		label,
		value,
		names,
		readonly = false,
		onchange,
		preview,
		check,
		lines = 4
	}: {
		id: string;
		/** Its accessible name (the field's label). */
		label: string;
		value: string;
		names: Suggestion[];
		readonly?: boolean;
		onchange: (value: string) => void;
		/** Evaluates the expression against the latest run. */
		preview?: (expression: unknown) => Promise<PreviewResult>;
		/** The server's problems with this field, were it this text. */
		check?: (text: string) => Promise<string[]>;
		/** Height in lines before it scrolls. */
		lines?: number;
	} = $props();

	let text = $state(untrack(() => value));
	let committed = untrack(() => value);
	let timer: ReturnType<typeof setTimeout> | undefined;

	// A new value from outside (another step selected, undo) replaces ours.
	$effect(() => {
		const v = value;
		untrack(() => {
			if (v !== committed) {
				committed = v;
				text = v;
			}
		});
	});

	function commit() {
		clearTimeout(timer);
		if (text !== committed) {
			committed = text;
			onchange(text);
		}
	}
	function edited() {
		clearTimeout(timer);
		timer = setTimeout(commit, 400);
	}
	onDestroy(commit);

	const completions = (doc: string, pos: number) => {
		const found = complete(doc, pos, names);
		if (!found) return null;
		return {
			from: found.from,
			to: found.to,
			options: found.items.map((s) => ({ label: s.label, detail: s.detail }))
		};
	};

	const lint = $derived(
		check
			? async (doc: string): Promise<CodeDiagnostic[]> =>
					(await check(doc)).map((message) => ({
						from: 0,
						to: doc.length,
						severity: 'error' as const,
						message
					}))
			: undefined
	);
</script>

<div class="flex flex-col gap-1" {id}>
	<CodeEditor
		{label}
		language="jinja"
		compact
		{lines}
		{readonly}
		complete={completions}
		{lint}
		onchange={edited}
		onblur={commit}
		bind:value={text}
	/>
	{#if preview && text.includes('{{')}
		<PreviewButton {preview} value={() => text} />
	{/if}
</div>
