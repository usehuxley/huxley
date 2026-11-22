<script lang="ts" module>
	export type Column<R> = {
		key: string;
		header: string;
		/** Plain-text cell value. Use the `cell` snippet for rich content. */
		value?: (row: R) => string | number | null | undefined;
		align?: 'left' | 'right';
		class?: string;
	};
</script>

<script lang="ts" generics="Row">
	import type { Snippet } from 'svelte';

	type Props = {
		columns: Column<Row>[];
		rows: Row[];
		/** Stable key per row for keyed rendering. */
		rowKey: (row: Row) => string;
		/** Optional rich cell renderer: (row, column key). */
		cell?: Snippet<[Row, string]>;
		/** Shown when `rows` is empty. */
		empty?: Snippet;
		caption?: string;
	};

	let { columns, rows, rowKey, cell, empty, caption }: Props = $props();
</script>

<div class="overflow-x-auto rounded-card border border-border bg-surface">
	<table class="w-full text-left text-sm">
		{#if caption}<caption class="sr-only">{caption}</caption>{/if}
		<thead
			class="border-b border-border bg-surface-sunken text-xs tracking-wide text-fg-muted uppercase"
		>
			<tr>
				{#each columns as column (column.key)}
					<th
						scope="col"
						class="px-4 py-2.5 font-medium {column.align === 'right' ? 'text-right' : ''}"
					>
						{column.header}
					</th>
				{/each}
			</tr>
		</thead>
		<tbody>
			{#each rows as row (rowKey(row))}
				<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
					{#each columns as column (column.key)}
						<td
							class="px-4 py-2.5 {column.align === 'right' ? 'text-right' : ''} {column.class ??
								''}"
						>
							{#if cell}
								{@render cell(row, column.key)}
							{:else}
								{column.value?.(row) ?? ''}
							{/if}
						</td>
					{/each}
				</tr>
			{:else}
				<tr>
					<td colspan={columns.length} class="px-4 py-10 text-center text-fg-muted">
						{#if empty}{@render empty()}{:else}No results.{/if}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
