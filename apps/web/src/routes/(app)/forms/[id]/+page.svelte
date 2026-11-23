<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Button, Card, Field, Select } from '@usehuxley/ui';
	import FormRenderer from '#lib/form-renderer/FormRenderer.svelte';
	import { answersOf, fieldsOf } from '#lib/form-renderer/fields.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let values = $state<Record<string, unknown>>({});
	let fields = $derived(fieldsOf(data.form.schema, data.form.ui));
	let answers = $derived(JSON.stringify(answersOf(fields, values)));
	let orgId = $state('');
	$effect(() => {
		if (!orgId) orgId = data.form.orgId;
	});
	let orgOptions = $derived(data.orgs.map((o) => ({ value: o.id, label: o.name })));
	let errors: Record<string, string> = $derived(
		form && 'errors' in form && form.errors ? (form.errors as Record<string, string>) : {}
	);
</script>

<svelte:head>
	<title>{data.form.name} · Forms · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-2xl flex-col gap-6">
	<header>
		<a
			href={resolve('/(app)/forms')}
			class="inline-flex items-center gap-1 text-sm text-fg-muted hover:text-fg"
		>
			<ArrowLeft size={14} /> Forms
		</a>
		<h1 class="mt-2 text-2xl font-semibold text-fg">{data.form.name}</h1>
		{#if data.form.description}<p class="mt-1 text-sm text-fg-muted">
				{data.form.description}
			</p>{/if}
	</header>

	{#if form && 'submitted' in form && form.submitted}
		<Card title="Submitted">
			<div class="flex flex-col gap-2 text-sm" role="status">
				{#if form.submitted.runs.length === 0}
					<p class="text-fg">Saved. No workflows are listening for this form yet.</p>
				{:else}
					<p class="text-fg">Started {form.submitted.runs.length} workflow run(s):</p>
					<ul class="list-disc pl-5">
						{#each form.submitted.runs as run (run)}
							<li>
								<a
									href={resolve('/(app)/runs/[id]', { id: run })}
									class="text-accent hover:underline"
								>
									View run
								</a>
							</li>
						{/each}
					</ul>
				{/if}
				{#if form.submitted.skipped.length}
					<p class="text-danger">
						Not started (their inputs don't match this form): {form.submitted.skipped.join(', ')}
					</p>
				{/if}
			</div>
		</Card>
	{:else}
		<Card>
			<form
				method="POST"
				action="?/submit"
				class="flex flex-col gap-4"
				use:enhance={() =>
					async ({ update }) => {
						await update({ reset: false });
					}}
			>
				<Field label="For" for="orgId" hint="The organization this is for">
					<Select id="orgId" name="orgId" options={orgOptions} bind:value={orgId} />
				</Field>
				<FormRenderer schema={data.form.schema} ui={data.form.ui} bind:values {errors} />
				<input type="hidden" name="answers" value={answers} />
				<div><Button type="submit" disabled={data.form.archived}>Submit</Button></div>
				{#if form && 'message' in form && form.message}
					<p class="text-sm text-danger" role="alert">{form.message}</p>
				{/if}
			</form>
		</Card>
	{/if}
</div>
