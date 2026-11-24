<!--
	"Preview with the last run": evaluates a field's value (a string with
	expressions, or a JSON value holding some) against the workflow's latest
	run, and shows what it gives.
-->
<script lang="ts">
	import type { PreviewResult } from './context.ts';

	let {
		preview,
		value
	}: {
		preview: (expression: unknown) => Promise<PreviewResult>;
		/** The field's current value. */
		value: () => unknown;
	} = $props();

	let previewed = $state<PreviewResult | null>(null);
	let previewing = $state(false);
	async function show() {
		previewing = true;
		try {
			previewed = await preview(value());
		} catch (e) {
			previewed = { error: e instanceof Error ? e.message : String(e) };
		} finally {
			previewing = false;
		}
	}
	const shown = (v: unknown) => (typeof v === 'string' ? v : JSON.stringify(v, null, 2));
</script>

<button
	type="button"
	class="mt-1 self-start text-xs text-accent hover:underline"
	onclick={show}
	disabled={previewing}
>
	{previewing ? 'Previewing…' : 'Preview with the last run'}
</button>
{#if previewed}
	{#if previewed.error}
		<p class="mt-1 text-xs text-danger" role="status">{previewed.error}</p>
	{:else}
		<pre
			class="mt-1 max-h-40 overflow-auto rounded-control bg-surface-sunken p-2 font-mono text-xs text-fg"
			role="status"
			aria-label="Preview">{shown(previewed.value)}</pre>
	{/if}
{/if}
