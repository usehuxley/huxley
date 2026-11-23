<script lang="ts">
	import type { components } from '@usehuxley/api-client';

	type Permission = components['schemas']['Permission'];

	let {
		permissions,
		selected = [],
		held
	}: {
		permissions: Permission[];
		selected?: string[];
		/** Permissions the signed-in user holds; others can't be granted. */
		held: string[];
	} = $props();
</script>

<fieldset class="grid gap-1.5 sm:grid-cols-2">
	<legend class="mb-2 text-sm text-fg-muted">
		You can only include permissions you hold yourself.
	</legend>
	{#each permissions as p (p.key)}
		{@const can = held.includes(p.key)}
		<label class="flex items-start gap-2 text-sm {can ? 'text-fg' : 'text-fg-subtle'}">
			<input
				type="checkbox"
				name="permissions"
				value={p.key}
				checked={selected.includes(p.key) || selected.includes('*')}
				disabled={!can && !selected.includes(p.key)}
				class="mt-0.5"
			/>
			<span>
				{p.description}
				<code class="block font-mono text-xs text-fg-muted">{p.key}</code>
			</span>
		</label>
	{/each}
</fieldset>
