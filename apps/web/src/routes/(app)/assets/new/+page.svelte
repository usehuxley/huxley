<script lang="ts">
	import { resolve } from '$app/paths';
	import { Button, Card, Field, Select } from '@usehuxley/ui';
	import AssetForm from '#lib/assets/AssetForm.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>New asset · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<header>
		<p class="text-sm">
			<a class="text-fg-muted hover:text-fg" href={resolve('/(app)/assets')}>Assets</a>
		</p>
		<h1 class="text-2xl font-semibold text-fg">
			New asset{data.orgName ? ` for ${data.orgName}` : ''}
		</h1>
	</header>
	{#if !data.orgId}
		<Card>
			<form method="GET" class="flex items-end gap-2">
				<Field label="Whose asset is it?" for="asset-org">
					<Select id="asset-org" name="org" options={data.orgs} />
				</Field>
				<Button type="submit">Continue</Button>
			</form>
		</Card>
	{:else}
		{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
		<Card>
			<AssetForm
				types={data.types}
				sites={data.sites}
				contacts={data.contacts}
				errors={form?.errors ?? {}}
				submitLabel="Add asset"
				action={`?org=${data.orgId}&/save`}
			/>
		</Card>
	{/if}
</div>
