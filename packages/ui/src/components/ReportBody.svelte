<script lang="ts">
	// Short reports in a small Markdown subset: paragraphs, a line that is
	// all **bold** as a heading, and "- " bullets. Rendered as text, so
	// nothing in it is ever HTML.
	let { body }: { body: string } = $props();
	type Block = { kind: 'heading' | 'paragraph'; text: string } | { kind: 'list'; items: string[] };
	let blocks = $derived.by(() => {
		const out: Block[] = [];
		for (const chunk of body.split(/\n\s*\n/)) {
			const lines = chunk
				.split('\n')
				.map((l) => l.trim())
				.filter(Boolean);
			if (lines.length === 0) continue;
			if (lines.every((l) => l.startsWith('- '))) {
				out.push({ kind: 'list', items: lines.map((l) => l.slice(2)) });
			} else if (lines.length === 1 && /^\*\*.+\*\*$/.test(lines[0])) {
				out.push({ kind: 'heading', text: lines[0].slice(2, -2) });
			} else {
				out.push({ kind: 'paragraph', text: lines.join(' ') });
			}
		}
		return out;
	});
</script>

<div class="flex flex-col gap-2 text-sm text-fg">
	{#each blocks as b, i (i)}
		{#if b.kind === 'heading'}
			<p class="font-semibold">{b.text}</p>
		{:else if b.kind === 'list'}
			<ul class="list-disc pl-5">
				{#each b.items as item, j (j)}<li>{item}</li>{/each}
			</ul>
		{:else}
			<p>{b.text}</p>
		{/if}
	{/each}
</div>
