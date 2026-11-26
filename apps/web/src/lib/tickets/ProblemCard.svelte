<!-- A problem ticket's root cause, workaround and known-error status. -->
<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, Field, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let {
		problem,
		canWrite,
		canDraft
	}: { problem: components['schemas']['ProblemRecord']; canWrite: boolean; canDraft: boolean } =
		$props();

	const done =
		(message: string): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') toast.push({ title: message, tone: 'success' });
			else if (result.type === 'failure') {
				const data = result.data as { problem?: { message?: string } } | undefined;
				toast.push({ title: data?.problem?.message ?? 'That didn’t work.', tone: 'danger' });
			}
		};
</script>

<Card title="Problem">
	<div class="flex flex-wrap items-center gap-2">
		{#if problem.knownError}<Badge tone="warning">Known error</Badge>{:else}<Badge>
				Investigating
			</Badge>{/if}
		<span class="text-sm text-fg-muted">
			{problem.incidents} incident{problem.incidents === 1 ? '' : 's'} linked
		</span>
	</div>
	{#if canWrite}
		<form
			method="POST"
			action="?/saveProblem"
			class="mt-3 flex flex-col gap-3"
			use:enhance={done('Problem saved.')}
		>
			<input type="hidden" name="version" value={problem.version} />
			<Field label="Root cause" for="pb-cause">
				<textarea
					id="pb-cause"
					name="rootCause"
					rows="3"
					class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
					>{problem.rootCause}</textarea
				>
			</Field>
			<Field label="Workaround" for="pb-workaround">
				<textarea
					id="pb-workaround"
					name="workaround"
					rows="3"
					class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
					>{problem.workaround}</textarea
				>
			</Field>
			<label class="flex items-center gap-2 text-sm text-fg">
				<input type="checkbox" name="knownError" checked={problem.knownError} />
				Known error (the cause is known and there's a workaround)
			</label>
			<div><Button type="submit">Save problem</Button></div>
		</form>
		{#if canDraft && problem.workaround && !problem.kbArticleId}
			<form
				method="POST"
				action="?/problemArticle"
				class="mt-2"
				use:enhance={done('Article drafted and sent for review.')}
			>
				<Button type="submit" variant="ghost">Draft a knowledge article from the workaround</Button>
			</form>
		{/if}
	{:else}
		<dl class="mt-3 flex flex-col gap-1 text-sm">
			<dt class="text-fg-muted">Root cause</dt>
			<dd class="whitespace-pre-line text-fg">{problem.rootCause || '–'}</dd>
			<dt class="text-fg-muted">Workaround</dt>
			<dd class="whitespace-pre-line text-fg">{problem.workaround || '–'}</dd>
		</dl>
	{/if}
	{#if problem.kbArticleId}
		<p class="mt-2 text-sm">
			<a
				href={resolve('/(app)/knowledge/[id]', { id: problem.kbArticleId })}
				class="text-accent hover:underline"
			>
				The known-error article
			</a>
		</p>
	{/if}
	<p class="mt-2 text-xs text-fg-subtle">Link incidents below as “Incident (caused by this)”.</p>
</Card>
