<script lang="ts">
	import { formatValue, type Answer } from '../data';

	let { answer, label }: { answer: Answer; label: string } = $props();
</script>

<div class="max-h-80 overflow-auto">
	<table class="w-full text-sm" aria-label={label}>
		<thead class="sticky top-0 bg-surface">
			<tr class="border-b border-border text-left text-xs text-fg-muted">
				{#each answer.columns as c (c.key)}
					<th scope="col" class="py-1.5 pr-3 font-medium {c.kind === 'number' ? 'text-right' : ''}">
						{c.label}
					</th>
				{/each}
			</tr>
		</thead>
		<tbody>
			{#each answer.rows as row, i (i)}
				<tr class="border-b border-border last:border-0">
					{#each answer.columns as c, j (c.key)}
						<td class="py-1.5 pr-3 text-fg {c.kind === 'number' ? 'text-right tabular-nums' : ''}">
							{formatValue(row[j], c.kind)}
						</td>
					{/each}
				</tr>
			{/each}
		</tbody>
	</table>
</div>
