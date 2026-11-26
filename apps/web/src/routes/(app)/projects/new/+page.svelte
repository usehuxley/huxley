<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Card, Field, Input, Select } from '@usehuxley/ui';
	import { BILLING } from '#lib/projects/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let templateId = $state('');
	let template = $derived(data.templates.find((t) => t.id === templateId));
	const today = new Date().toISOString().slice(0, 10);
</script>

<svelte:head><title>New project · Huxley</title></svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-4">
	<div>
		<a href={resolve('/(app)/projects')} class="text-sm text-accent hover:underline">Projects</a>
		<h1 class="mt-1 text-2xl font-semibold text-fg">New project</h1>
	</div>
	<Card>
		<form method="POST" class="flex flex-col gap-4" use:enhance>
			<div class="grid gap-3 sm:grid-cols-2">
				<Field label="Client" for="pj-org">
					<Select id="pj-org" name="orgId" options={data.companies} />
				</Field>
				<Field label="Name" for="pj-name"><Input id="pj-name" name="name" required /></Field>
				<Field label="Starts" for="pj-start">
					<Input id="pj-start" name="startOn" type="date" value={today} required />
				</Field>
				<Field label="Ends (optional)" for="pj-end">
					<Input id="pj-end" name="endOn" type="date" />
				</Field>
				<Field label="Template" for="pj-template">
					<Select
						id="pj-template"
						name="templateId"
						bind:value={templateId}
						options={[
							{ value: '', label: 'None: start empty' },
							...data.templates.map((t) => ({ value: t.id, label: t.name }))
						]}
					/>
				</Field>
				<Field label="Budget hours" for="pj-budget">
					<Input
						id="pj-budget"
						name="budgetHours"
						type="number"
						min="0"
						step="0.5"
						placeholder={template ? 'From the template' : ''}
					/>
				</Field>
				<Field label="Billing" for="pj-billing">
					<Select
						id="pj-billing"
						name="billingMethod"
						options={BILLING.map((b) => ({ value: b.value, label: b.label }))}
					/>
				</Field>
			</div>
			<Field label="Description" for="pj-description">
				<textarea
					id="pj-description"
					name="description"
					rows="2"
					class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
				></textarea>
			</Field>
			{#if template}
				<fieldset class="flex flex-col gap-2" aria-label="Who does what">
					<legend class="text-sm font-medium text-fg">Who does what</legend>
					{#each template.structure.phases ?? [] as phase (phase.name)}
						<p class="text-xs font-semibold text-fg-muted uppercase">{phase.name}</p>
						{#each phase.tasks ?? [] as task (task.key)}
							<div class="grid items-center gap-2 sm:grid-cols-[1fr_16rem]">
								<label for="as-{task.key}" class="text-sm text-fg">{task.title}</label>
								<Select
									id="as-{task.key}"
									name="assign.{task.key}"
									options={[{ value: '', label: 'Unassigned' }, ...data.people]}
								/>
							</div>
						{/each}
					{/each}
				</fieldset>
			{/if}
			{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
			<div><Button type="submit" variant="primary">Create project</Button></div>
		</form>
	</Card>
</div>
