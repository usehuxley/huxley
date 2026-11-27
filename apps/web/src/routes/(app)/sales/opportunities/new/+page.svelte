<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Card, Field, Select } from '@usehuxley/ui';
	import OpportunityFields from '#lib/sales/OpportunityFields.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let errors = $derived(form?.errors ?? {});
</script>

<svelte:head>
	<title>New opportunity · Sales · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-6">
	<div>
		<a href={resolve('/(app)/sales')} class="text-sm text-fg-muted hover:text-fg">← Pipeline</a>
		<h1 class="mt-2 text-[26px] font-extrabold tracking-tight text-fg">New opportunity</h1>
	</div>
	<Card>
		<form method="POST" class="flex flex-col gap-4" use:enhance>
			<div class="grid gap-4 sm:grid-cols-2">
				<Field label="Client" for="orgId" error={errors.orgId}>
					<Select
						id="orgId"
						name="orgId"
						required
						value={data.orgId}
						placeholder="Choose a client"
						options={data.clients}
					/>
				</Field>
				<Field label="Stage" for="stageId">
					<Select id="stageId" name="stageId" options={data.stages} />
				</Field>
			</div>
			<OpportunityFields people={data.people} {errors} />
			{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
			<div><Button type="submit" variant="primary">Create opportunity</Button></div>
		</form>
	</Card>
</div>
