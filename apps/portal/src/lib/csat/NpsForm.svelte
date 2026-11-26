<!-- Asks the net promoter question (0 to 10, and an optional comment); posts to `action`. -->
<script lang="ts">
	import { enhance } from '$app/forms';
	import { untrack } from 'svelte';
	import { Button, Field } from '@usehuxley/ui';

	type Props = {
		action: string;
		company: string;
		selected?: number | null;
		message?: string | null;
	};
	let { action, company, selected = null, message = null }: Props = $props();
	let score = $state<number | null>(untrack(() => selected));
</script>

<form method="POST" {action} class="flex flex-col gap-4" use:enhance>
	<fieldset>
		<legend class="mb-2 text-sm font-medium text-fg">
			How likely are you to recommend {company} to a friend or colleague?
		</legend>
		<div class="flex flex-wrap gap-1.5">
			{#each Array.from({ length: 11 }, (_, n) => n) as n (n)}
				<label
					class="flex size-9 cursor-pointer items-center justify-center rounded-md border text-sm {score ===
					n
						? 'border-accent bg-accent-soft text-fg'
						: 'border-border text-fg-muted hover:border-accent'}"
				>
					<input
						type="radio"
						name="score"
						value={n}
						bind:group={score}
						class="sr-only"
						aria-label={String(n)}
					/>
					{n}
				</label>
			{/each}
		</div>
		<p class="mt-1 flex justify-between text-xs text-fg-subtle">
			<span>Not at all likely</span><span>Extremely likely</span>
		</p>
	</fieldset>
	<Field label="What’s the main reason for your score? (optional)" for="nps-comment">
		<textarea
			id="nps-comment"
			name="comment"
			rows="3"
			maxlength="4000"
			class="w-full rounded-md border border-border bg-surface px-3 py-2 text-sm text-fg"
		></textarea>
	</Field>
	{#if message}<p class="text-sm text-danger" role="alert">{message}</p>{/if}
	<div><Button type="submit" variant="primary" disabled={score === null}>Send</Button></div>
</form>
