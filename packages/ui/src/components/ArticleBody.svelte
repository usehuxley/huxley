<script lang="ts">
	/** An article's text: HTML the API rendered from markdown, safe to show
	 * (raw HTML is escaped and only safe link addresses are kept). */
	let { html, label = 'Article' }: { html: string; label?: string } = $props();
</script>

<div class="kb-article" aria-label={label} role="document">
	<!-- eslint-disable-next-line svelte/no-at-html-tags -- rendered and made safe by the API -->
	{@html html}
</div>

<style>
	.kb-article {
		color: var(--color-fg);
		font-size: 0.9375rem;
		line-height: 1.65;
		overflow-wrap: anywhere;
	}
	.kb-article :global(h1),
	.kb-article :global(h2),
	.kb-article :global(h3),
	.kb-article :global(h4) {
		font-weight: 600;
		line-height: 1.3;
		margin: 1.4em 0 0.5em;
	}
	.kb-article :global(h1) {
		font-size: 1.5rem;
	}
	.kb-article :global(h2) {
		font-size: 1.25rem;
	}
	.kb-article :global(h3) {
		font-size: 1.0625rem;
	}
	.kb-article :global(:first-child) {
		margin-top: 0;
	}
	.kb-article :global(p),
	.kb-article :global(ul),
	.kb-article :global(ol),
	.kb-article :global(pre),
	.kb-article :global(table),
	.kb-article :global(blockquote) {
		margin: 0 0 0.9em;
	}
	.kb-article :global(ul) {
		list-style: disc;
		padding-left: 1.5em;
	}
	.kb-article :global(ol) {
		list-style: decimal;
		padding-left: 1.5em;
	}
	.kb-article :global(li) {
		margin: 0.2em 0;
	}
	.kb-article :global(a) {
		color: var(--color-accent);
		text-decoration: underline;
	}
	.kb-article :global(code) {
		font-family: var(--font-mono, ui-monospace, monospace);
		font-size: 0.875em;
		background: var(--color-surface-sunken);
		border-radius: 4px;
		padding: 0.1em 0.3em;
	}
	.kb-article :global(pre) {
		background: var(--color-surface-sunken);
		border: 1px solid var(--color-border);
		border-radius: 6px;
		padding: 0.75em 1em;
		overflow-x: auto;
	}
	.kb-article :global(pre code) {
		background: none;
		padding: 0;
	}
	.kb-article :global(blockquote) {
		border-left: 3px solid var(--color-border-strong);
		color: var(--color-fg-muted);
		padding-left: 1em;
	}
	.kb-article :global(table) {
		border-collapse: collapse;
		display: block;
		overflow-x: auto;
	}
	.kb-article :global(th),
	.kb-article :global(td) {
		border: 1px solid var(--color-border);
		padding: 0.35em 0.7em;
		text-align: left;
	}
	.kb-article :global(th) {
		background: var(--color-surface-sunken);
		font-weight: 600;
	}
	.kb-article :global(img) {
		max-width: 100%;
	}
	.kb-article :global(hr) {
		border: 0;
		border-top: 1px solid var(--color-border);
		margin: 1.5em 0;
	}
</style>
