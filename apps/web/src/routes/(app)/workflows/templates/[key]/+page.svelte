<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Button, Card, Field, Input, Select } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let orgId = $state('');
	$effect(() => {
		if (!orgId) orgId = data.orgs[0]?.id ?? '';
	});
	let orgOptions = $derived(data.orgs.map((o) => ({ value: o.id, label: o.name })));
	let errors: Record<string, string> = $derived(
		form && 'errors' in form && form.errors ? (form.errors as Record<string, string>) : {}
	);
</script>

<svelte:head>
	<title>{data.template.name} · Templates · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-2xl flex-col gap-6">
	<header>
		<a
			href={resolve('/(app)/workflows/templates')}
			class="inline-flex items-center gap-1 text-sm text-fg-muted hover:text-fg"
		>
			<ArrowLeft size={14} /> Templates
		</a>
		<h1 class="mt-2 text-2xl font-semibold text-fg">{data.template.name}</h1>
		<p class="mt-1 text-sm text-fg-muted">{data.template.description}</p>
	</header>
	<Card
		title="Use this template"
		description={data.template.hasForm
			? 'Creates a draft workflow, and the form that starts it.'
			: 'Creates a draft workflow to review and publish.'}
	>
		<form method="POST" use:enhance class="flex flex-col gap-4">
			<Field label="Name" for="name">
				<Input id="name" name="name" required maxlength={200} value={data.template.name} />
			</Field>
			<Field label="Key" for="key" hint="Optional; made from the name">
				<Input id="key" name="key" pattern="[a-z][a-z0-9-]*" maxlength={64} />
			</Field>
			<Field label="Organization" for="orgId">
				<Select id="orgId" name="orgId" options={orgOptions} bind:value={orgId} />
			</Field>
			{#each data.template.bindings as binding (binding.key)}
				<Field
					label={binding.label}
					for={`binding-${binding.key}`}
					hint={binding.help ?? undefined}
					error={errors[binding.key]}
				>
					<Input
						id={`binding-${binding.key}`}
						name={`binding.${binding.key}`}
						value={binding.default ?? ''}
						required={!binding.default}
					/>
				</Field>
			{/each}
			<div><Button type="submit" variant="primary">Create draft</Button></div>
			{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
		</form>
	</Card>
</div>
