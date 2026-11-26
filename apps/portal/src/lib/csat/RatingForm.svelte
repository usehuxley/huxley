<!-- Asks for a rating (and an optional comment); posts to `action`. -->
<script lang="ts">
	import { enhance } from '$app/forms';
	import { untrack } from 'svelte';
	import { Button, Field } from '@usehuxley/ui';
	import { choices } from './choices.ts';

	type Props = { scale: string; action: string; selected?: number | null; message?: string | null };
	let { scale, action, selected = null, message = null }: Props = $props();
	let rating = $state<number | null>(untrack(() => selected));
</script>

<form method="POST" {action} class="flex flex-col gap-4" use:enhance>
	<fieldset>
		<legend class="mb-2 text-sm font-medium text-fg">How did we do?</legend>
		<div class="flex flex-wrap gap-2">
			{#each choices(scale) as c (c.value)}
				<label
					class="cursor-pointer rounded-md border px-3 py-2 text-sm {rating === c.value
						? 'border-accent bg-accent-soft text-fg'
						: 'border-border text-fg-muted hover:border-accent'}"
				>
					<input type="radio" name="rating" value={c.value} bind:group={rating} class="sr-only" />
					{c.label}
				</label>
			{/each}
		</div>
	</fieldset>
	<Field label="Anything to add? (optional)" for="csat-comment">
		<textarea
			id="csat-comment"
			name="comment"
			rows="3"
			maxlength="4000"
			class="w-full rounded-md border border-border bg-surface px-3 py-2 text-sm text-fg"
		></textarea>
	</Field>
	{#if message}<p class="text-sm text-danger" role="alert">{message}</p>{/if}
	<div><Button type="submit" variant="primary" disabled={rating === null}>Send</Button></div>
</form>
