<script lang="ts">
	import { resolve } from '$app/paths';
	import { Card } from '@usehuxley/ui';
	import AssetForm from '#lib/assets/AssetForm.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>Edit {data.asset.name} · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<header>
		<p class="text-sm">
			<a
				class="text-fg-muted hover:text-fg"
				href={resolve('/(app)/assets/[id]', { id: data.asset.id })}
			>
				{data.asset.name}
			</a>
		</p>
		<h1 class="text-2xl font-semibold text-fg">Edit asset</h1>
	</header>
	{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
	<Card>
		<AssetForm
			asset={data.asset}
			types={data.types}
			sites={data.sites}
			contacts={data.contacts}
			errors={form?.errors ?? {}}
			submitLabel="Save"
		/>
	</Card>
</div>
